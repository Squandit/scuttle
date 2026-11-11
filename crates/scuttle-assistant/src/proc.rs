//! Starting other programs.

use std::io;
use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

/// The error Linux gives when a program file is still open for
/// writing somewhere: "Text file busy".
const ETXTBSY: i32 = 26;

/// Starts a program, trying again for a moment if its file is busy.
///
/// That happens while a program is being replaced by an update, and
/// for an instant after it is written. Either way it clears by itself.
pub fn spawn(command: &mut Command) -> io::Result<Child> {
    let mut tries = 0;
    loop {
        match command.spawn() {
            Err(e) if e.raw_os_error() == Some(ETXTBSY) && tries < 20 => {
                tries += 1;
                thread::sleep(Duration::from_millis(15));
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_missing_program_is_still_an_error() {
        let error = spawn(&mut Command::new("/nonexistent/scuttle/program")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_program_whose_file_is_busy_starts_once_it_is_free() {
        let dir = std::env::temp_dir().join(format!("scuttle-proc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("busy");
        std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Hold the file open for writing, then let go shortly after.
        let holder = std::fs::OpenOptions::new()
            .append(true)
            .open(&script)
            .unwrap();
        assert_eq!(
            Command::new(&script).spawn().unwrap_err().raw_os_error(),
            Some(ETXTBSY)
        );
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(60));
            drop(holder);
        });
        let mut child = spawn(&mut Command::new(&script)).unwrap();
        assert!(child.wait().unwrap().success());
        release.join().unwrap();
    }
}
