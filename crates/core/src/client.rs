//! A small blocking client. The hook and the CLI need no async runtime, and
//! the TUI runs it on a background thread.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::protocol::{Event, Request, Response, encode};

pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        let writer = stream.try_clone()?;
        Ok(Client {
            reader: BufReader::new(stream),
            writer,
        })
    }

    /// How long a call may wait for its response; `None` waits forever.
    pub fn set_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.writer.set_read_timeout(timeout)
    }

    pub fn call(&mut self, req: &Request) -> io::Result<Response> {
        self.writer.write_all(&encode(req))?;
        self.read()
    }

    fn read(&mut self) -> io::Result<Response> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "daemon closed the connection",
            ));
        }
        serde_json::from_str(&line).map_err(io::Error::other)
    }

    /// Turns the connection into an event stream.
    pub fn subscribe(mut self) -> io::Result<impl Iterator<Item = Event>> {
        self.set_timeout(None)?;
        match self.call(&Request::Subscribe)? {
            Response::Ok => {}
            other => return Err(io::Error::other(format!("subscribe refused: {other:?}"))),
        }
        Ok(std::iter::from_fn(move || {
            loop {
                match self.read() {
                    Ok(Response::Event { event }) => return Some(event),
                    Ok(_) => continue,
                    Err(_) => return None,
                }
            }
        }))
    }
}
