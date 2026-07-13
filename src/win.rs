//! Windows elevation via UAC.
//!
//! There is no `sudo` on Windows, so we request elevation through PowerShell's
//! `Start-Process -Verb RunAs`, which triggers the UAC consent/credential
//! prompt on the secure desktop. Because the elevated process is separate and
//! UAC can't display our command text, we show the preview as a message box
//! first. Note: the elevated command runs in its own window and its stdout is
//! not captured here — we only recover its exit code.

use std::process::{Command, Stdio};

pub fn elevate(cmd: &[String], preview: &str) -> i32 {
    use std::io::IsTerminal;
    let interactive = Some(std::io::stdin().is_terminal());
    if !confirm(preview, &caller(), interactive) {
        eprintln!("vudo: cancelled");
        return 130;
    }

    let file = ps_quote(&cmd[0]);
    let script = if cmd.len() > 1 {
        let arg_list = cmd[1..]
            .iter()
            .map(|a| ps_quote(a))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "$p = Start-Process -FilePath {file} -ArgumentList {arg_list} \
             -Verb RunAs -PassThru -Wait; exit $p.ExitCode"
        )
    } else {
        format!("$p = Start-Process -FilePath {file} -Verb RunAs -PassThru -Wait; exit $p.ExitCode")
    };

    match Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
    {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("vudo: failed to launch PowerShell: {e}");
            127
        }
    }
}

/// Best-effort caller name, so the user can see what asked for elevation:
/// walk the parent chain via CIM and let the shared picker choose the nearest
/// non-shell ancestor. Falls back to "unknown".
fn caller() -> String {
    let mypid = std::process::id();
    // Emit each ancestor's name on its own line, parent-first, skipping vudo
    // itself (d=0). The Rust side picks the first non-shell.
    let script = format!(
        "$id={mypid}; $d=0; \
         while ($d -lt 8) {{ \
           $p = Get-CimInstance Win32_Process -Filter \"ProcessId=$id\" -ErrorAction SilentlyContinue; \
           if (-not $p) {{ break }}; \
           if ($d -ge 1) {{ Write-Output $p.Name }}; \
           $id = $p.ParentProcessId; \
           if (-not $id) {{ break }}; \
           $d++ \
         }}"
    );
    match Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
    {
        Ok(o) if o.status.success() => {
            let chain: Vec<String> = String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            crate::caller::pick(&chain)
        }
        _ => "unknown".to_string(),
    }
}

fn confirm(preview: &str, caller: &str, interactive: Option<bool>) -> bool {
    // Windows runs administrator, not "root"; the message box is plain text, so
    // keep it ASCII (drop the ⚠/· used on Unix). --cache is a no-op under UAC,
    // so never advertise caching here.
    let info = crate::dialog::info_block(preview, caller, interactive, false)
        .replace(
            "Run this command as root:",
            "Run this command as administrator:",
        )
        .replace('\u{26a0}', "(!)")
        .replace('\u{00b7}', "-");
    let msg = format!("{}`n`nProceed?", ps_multiline(&info));
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms | Out-Null; \
         $r = [System.Windows.Forms.MessageBox]::Show('{msg}', 'vudo', 'OKCancel', 'Warning'); \
         if ($r -eq 'OK') {{ exit 0 }} else {{ exit 1 }}"
    );
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// PowerShell single-quoted string literal (doubles embedded single quotes).
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Escape a value for embedding inside a single-quoted PS string, turning
/// newlines into PowerShell's `n line breaks.
fn ps_multiline(s: &str) -> String {
    s.replace('\'', "''")
        .replace("\r\n", "`n")
        .replace('\n', "`n")
        .replace('\r', "`n")
}
