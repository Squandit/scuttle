//! The control protocol between the `scuttle` command and the running
//! shell: one JSON object per line over a Unix socket.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A surface the shell can show or hide on request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    Launcher,
    Bar,
}

/// A two-state setting changed from the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Switch {
    On,
    Off,
    Toggle,
}

impl Switch {
    /// The new state, given the current one.
    pub fn apply(self, current: bool) -> bool {
        match self {
            Switch::On => true,
            Switch::Off => false,
            Switch::Toggle => !current,
        }
    }
}

/// Which way to move a level such as volume or brightness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    Up,
    Down,
    /// Toggle mute. Only meaningful for volume.
    Mute,
}

/// A command sent to the running shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Request {
    /// Check that the shell is alive.
    Ping,
    Show {
        surface: Surface,
    },
    Hide {
        surface: Surface,
    },
    Toggle {
        surface: Surface,
    },
    /// Open the launcher with text already typed, e.g. a mode prefix.
    Launcher {
        query: String,
    },
    /// Turn "do not disturb" on or off.
    Dnd {
        state: Switch,
    },
    /// Dismiss every notification on screen.
    Dismiss,
    /// Change the output volume by `amount` percentage points, or
    /// toggle mute.
    Volume {
        step: Step,
        amount: u32,
    },
    /// Change the screen brightness by `amount` percentage points.
    Brightness {
        step: Step,
        amount: u32,
    },
    /// Re-read the config file and restyle everything.
    Reload,
    /// Stop the shell.
    Quit,
}

/// The shell's answer to a [`Request`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Response {
    Ok,
    Pong { version: String },
    Error { message: String },
}

/// Sends one request to the shell listening on `socket` and waits for
/// its answer.
pub fn send(socket: &Path, request: &Request) -> io::Result<Response> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut line = serde_json::to_string(request).map_err(io::Error::other)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    serde_json::from_str(reply.trim()).map_err(io::Error::other)
}

/// Decodes one line received by the shell.
pub fn decode_request(line: &str) -> Result<Request, String> {
    serde_json::from_str(line.trim()).map_err(|e| format!("Unrecognised command: {e}"))
}

/// Encodes a response as a single line, newline included.
pub fn encode_response(response: &Response) -> String {
    let mut line = serde_json::to_string(response).expect("responses always serialise");
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        let all = [
            Request::Ping,
            Request::Show {
                surface: Surface::Launcher,
            },
            Request::Hide {
                surface: Surface::Bar,
            },
            Request::Toggle {
                surface: Surface::Launcher,
            },
            Request::Launcher {
                query: "= 2+2".into(),
            },
            Request::Dnd {
                state: Switch::Toggle,
            },
            Request::Dismiss,
            Request::Volume {
                step: Step::Up,
                amount: 5,
            },
            Request::Brightness {
                step: Step::Down,
                amount: 10,
            },
            Request::Reload,
            Request::Quit,
        ];
        for req in all {
            let line = serde_json::to_string(&req).unwrap();
            assert_eq!(decode_request(&line).unwrap(), req);
        }
    }

    #[test]
    fn wire_format_is_stable() {
        let line = serde_json::to_string(&Request::Toggle {
            surface: Surface::Launcher,
        })
        .unwrap();
        assert_eq!(line, r#"{"cmd":"toggle","surface":"launcher"}"#);
        assert_eq!(encode_response(&Response::Ok), "{\"status\":\"ok\"}\n");
    }

    #[test]
    fn switches_apply_to_the_current_state() {
        assert!(Switch::On.apply(false) && Switch::On.apply(true));
        assert!(!Switch::Off.apply(true) && !Switch::Off.apply(false));
        assert!(Switch::Toggle.apply(false) && !Switch::Toggle.apply(true));
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode_request("not json").is_err());
        assert!(decode_request(r#"{"cmd":"explode"}"#).is_err());
    }

    #[test]
    fn talks_over_a_real_socket() {
        use std::os::unix::net::UnixListener;
        let dir = std::env::temp_dir().join(format!("scuttle-ipc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(decode_request(&line).unwrap(), Request::Ping);
            let mut stream = stream;
            stream
                .write_all(
                    encode_response(&Response::Pong {
                        version: "t".into(),
                    })
                    .as_bytes(),
                )
                .unwrap();
        });
        let reply = send(&path, &Request::Ping).unwrap();
        assert_eq!(
            reply,
            Response::Pong {
                version: "t".into()
            }
        );
        server.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
