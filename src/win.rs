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
    if !confirm(preview, &caller()) {
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

/// Best-effort parent-process chain (starting from our own parent), so the
/// user can see what asked for elevation. Falls back to "unknown".
fn caller() -> String {
    let mypid = std::process::id();
    // Walk ParentProcessId up to 6 levels via CIM, skipping vudo itself (d=0).
    let script = format!(
        "$id={mypid}; $names=@(); $d=0; \
         while ($d -lt 6) {{ \
           $p = Get-CimInstance Win32_Process -Filter \"ProcessId=$id\" -ErrorAction SilentlyContinue; \
           if (-not $p) {{ break }}; \
           if ($d -ge 1) {{ $names += $p.Name }}; \
           $id = $p.ParentProcessId; \
           if (-not $id) {{ break }}; \
           $d++ \
         }}; \
         $names -join ' <- '"
    );
    match Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
    {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                "unknown".to_string()
            } else {
                s
            }
        }
        _ => "unknown".to_string(),
    }
}

fn confirm(preview: &str, caller: &str) -> bool {
    let msg = format!(
        "vudo will run this command as administrator:`n`n{}`n`nRequested by: {}`n`nProceed?",
        ps_literal(preview),
        ps_literal(caller)
    );
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

/// Escape a value for embedding inside an existing single-quoted PS string.
fn ps_literal(s: &str) -> String {
    s.replace('\'', "''").replace(['\r', '\n'], " ")
}
