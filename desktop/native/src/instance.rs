use crate::runtime::{read, save, Result};
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    path::Path,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};
pub struct Instance {
    _lock: File,
    pub requests: Receiver<Value>,
}
impl Instance {
    pub fn open(folder: &Path, args: Value) -> Result<Option<Self>> {
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(folder.join(".desktop.lock"))?;
        let endpoint = folder.join(".desktop.endpoint.json");
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                let start = Instant::now();
                loop {
                    if let Ok(ep) = read(&endpoint) {
                        if let (Some(port), Some(token)) = (
                            ep["port"].as_u64().and_then(|n| u16::try_from(n).ok()),
                            ep["token"].as_str(),
                        ) {
                            if let Ok(mut stream) = TcpStream::connect_timeout(
                                &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
                                Duration::from_millis(200),
                            ) {
                                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                                writeln!(stream, "{}", json!({"token":token,"args":args}))?;
                                let mut reply = [0; 2];
                                if stream.read_exact(&mut reply).is_ok() && reply == *b"OK" {
                                    return Ok(None);
                                }
                            }
                        }
                    }
                    if start.elapsed() > Duration::from_secs(3) {
                        return Err("The existing Robotutor window did not respond.".into());
                    }
                    thread::sleep(Duration::from_millis(60));
                }
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let token = uuid::Uuid::new_v4().to_string();
        save(
            &endpoint,
            &json!({"port":listener.local_addr()?.port(),"token":token}),
            false,
        )?;
        let (tx, requests) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                if stream
                    .set_read_timeout(Some(Duration::from_millis(300)))
                    .is_err()
                {
                    continue;
                }
                let mut line = String::new();
                if BufReader::new((&stream).take(8192))
                    .read_line(&mut line)
                    .is_err()
                    || !line.ends_with('\n')
                {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if v["token"].as_str() == Some(&token) && v["args"].is_object() {
                        if tx.send(v["args"].clone()).is_err() {
                            break;
                        }
                        let _ = stream.write_all(b"OK");
                    }
                }
            }
        });
        Ok(Some(Self {
            _lock: lock,
            requests,
        }))
    }
}
