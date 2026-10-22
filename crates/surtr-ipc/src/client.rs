//! The front-end side: a small blocking client.

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
    thread,
    time::Duration,
};

use serde_json::Value;

use crate::{Hello, PROTOCOL, Request, Response};

/// A connection to `surtr serve`.
pub struct Client {
    reader: BufReader<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
    next_id: u64,
    hello: Hello,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("hello", &self.hello)
            .finish()
    }
}

#[cfg(unix)]
fn open(endpoint: &Path) -> io::Result<(Box<dyn Read + Send>, Box<dyn Write + Send>)> {
    let stream = std::os::unix::net::UnixStream::connect(endpoint)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    Ok((Box::new(stream.try_clone()?), Box::new(stream)))
}

#[cfg(windows)]
fn open(endpoint: &Path) -> io::Result<(Box<dyn Read + Send>, Box<dyn Write + Send>)> {
    // ERROR_PIPE_BUSY: every instance is taken for a moment; try again.
    const PIPE_BUSY: i32 = 231;
    let mut attempts = 0;
    let file = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(endpoint)
        {
            Ok(f) => break f,
            Err(e) if e.raw_os_error() == Some(PIPE_BUSY) && attempts < 20 => {
                attempts += 1;
                thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(e),
        }
    };
    // The pipe name is guessable: another user could have created it first.
    if !crate::windows::server_is_ours(&file).unwrap_or(false) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is served by another user's process; not using it",
                endpoint.display()
            ),
        ));
    }
    Ok((Box::new(file.try_clone()?), Box::new(file)))
}

impl Client {
    /// Connects and checks the protocol version.
    pub fn connect(endpoint: &Path) -> io::Result<Self> {
        let (reader, writer) = open(endpoint)?;
        let mut client = Self {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
            hello: Hello {
                protocol: 0,
                version: String::new(),
                providers: Vec::new(),
            },
        };
        let hello: Hello = serde_json::from_value(client.call("hello").map_err(io::Error::other)?)
            .map_err(io::Error::other)?;
        if hello.protocol != PROTOCOL {
            return Err(io::Error::other(format!(
                "surtr serve speaks protocol {}, this surtr speaks {PROTOCOL}; restart surtr serve",
                hello.protocol
            )));
        }
        client.hello = hello;
        Ok(client)
    }

    /// The daemon's handshake answer.
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    /// Calls `method` and returns its result or the daemon's error.
    pub fn call(&mut self, method: &str) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = Request {
            id,
            method: method.to_owned(),
        };
        let mut text = serde_json::to_string(&request).map_err(|e| e.to_string())?;
        text.push('\n');
        self.writer
            .write_all(text.as_bytes())
            .and_then(|()| self.writer.flush())
            .map_err(|e| format!("lost the connection to surtr serve: {e}"))?;
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .map_err(|e| format!("lost the connection to surtr serve: {e}"))?;
        if n == 0 {
            return Err("surtr serve closed the connection".to_owned());
        }
        let response: Response = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        if response.id != id {
            return Err(format!(
                "response for request {} instead of {id}",
                response.id
            ));
        }
        match (response.ok, response.error) {
            (_, Some(e)) => Err(e),
            (Some(v), None) => Ok(v),
            (None, None) => Err("empty response".to_owned()),
        }
    }
}
