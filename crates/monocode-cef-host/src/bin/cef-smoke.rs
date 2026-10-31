//! Drives a host bundle the way the app does and saves the last frame it
//! receives: `cef-smoke "<dir>/MonoCode CEF.app" out.tga`.
#[path = "../frame_port.rs"]
mod frame_port;

use objc2_io_surface::{IOSurfaceLockOptions, IOSurfaceRef};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::{fd::FromRawFd, unix::process::CommandExt},
    time::{Duration, Instant},
};

fn main() {
    let mut args = std::env::args().skip(1);
    let app = std::path::PathBuf::from(args.next().expect("bundle path"));
    let out = args.next().unwrap_or_else(|| "frame.tga".into());
    let frames = frame_port::Receiver::new("smoke").expect("frame port");

    let pipe = || {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        for fd in fds {
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        (fds[0], fds[1])
    };
    let (commands_read, commands_write) = pipe();
    let (events_read, events_write) = pipe();
    let mut command = std::process::Command::new(app.join("Contents/MacOS/monocode-cef-host"));
    command.env("MONOCODE_CEF_FRAMES", frames.name());
    unsafe {
        command.pre_exec(move || {
            let commands = libc::fcntl(commands_read, libc::F_DUPFD, 10);
            let events = libc::fcntl(events_write, libc::F_DUPFD, 10);
            if libc::dup2(commands, 3) < 0 || libc::dup2(events, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        })
    };
    let started = Instant::now();
    let mut child = command.spawn().expect("host starts");
    unsafe {
        libc::close(commands_read);
        libc::close(events_write);
    }
    let mut write = unsafe { std::fs::File::from_raw_fd(commands_write) };
    let read = unsafe { std::fs::File::from_raw_fd(events_read) };
    let (events, received) = std::sync::mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let mut read = read;
        let mut buffer = Vec::new();
        let mut chunk = vec![0; 1 << 16];
        while let Ok(n) = read.read(&mut chunk) {
            if n == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..n]);
            while let Some(end) = buffer.iter().position(|b| *b == 0) {
                if let Ok(message) = serde_json::from_slice(&buffer[..end]) {
                    let _ = events.send(message);
                }
                buffer.drain(..=end);
            }
        }
    });
    let mut send = |message: Value| {
        let mut bytes = serde_json::to_vec(&message).unwrap();
        bytes.push(0);
        write.write_all(&bytes).unwrap();
    };
    let wait = |what: &str| -> Value {
        loop {
            let event = received
                .recv_timeout(Duration::from_secs(20))
                .unwrap_or_else(|_| panic!("no {what}"));
            if event.get(what).is_some() {
                return event;
            }
            eprintln!("event {event}");
        }
    };
    let ready = wait("ready");
    eprintln!("ready after {:?}: {ready}", started.elapsed());
    send(json!({"open": {"id": 1, "width": 400, "height": 300, "scale": 2}}));
    wait("opened");
    eprintln!("opened after {:?}", started.elapsed());
    let page = "<body style='margin:0;font:24px sans-serif'><div style='width:200px;height:100px;background:rgb(230,50,50)'></div><p style='color:rgb(50,100,255)'>Hello from CEF</p></body>";
    send(json!({"cdp": {"id": 1, "message": {"id": 1, "method": "Page.navigate", "params": {"url": format!("data:text/html,{page}")}}}}));
    let mut last = None;
    let mut count = 0;
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if let Some(frame) = frames.recv(Duration::from_millis(200)) {
            if count == 0 {
                eprintln!("first frame after {:?}: {}x{}", started.elapsed(), frame.width, frame.height);
            }
            count += 1;
            if let Some(old) = last.replace(frame) {
                frame_port::release(old.surface);
            }
        }
    }
    send(json!({"cdp": {"id": 1, "message": {"id": 2, "method": "Runtime.evaluate", "params": {"expression": "document.querySelector('p').textContent", "returnByValue": true}}}}));
    let reply = loop {
        let event = wait("cdp");
        if event["cdp"]["message"]["id"] == 2 {
            break event;
        }
    };
    eprintln!("evaluate: {}", reply["cdp"]["message"]);
    eprintln!("{count} frames");
    let frame = last.expect("a frame arrived");
    let surface = IOSurfaceRef::lookup_from_mach_port(frame.surface).expect("surface");
    frame_port::release(frame.surface);
    save(&surface, &out);
    eprintln!("saved {out}");
    drop(send);
    drop(write);
    let status = child.wait().unwrap();
    eprintln!("host exited {status}");
}

/// Writes the surface as an uncompressed 32-bit TGA, which stores BGRA.
fn save(surface: &IOSurfaceRef, out: &str) {
    unsafe { surface.lock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut()) };
    let (width, height, row) = (surface.width(), surface.height(), surface.bytes_per_row());
    let base = surface.base_address().as_ptr().cast::<u8>();
    let mut tga = vec![0u8; 18];
    tga[2] = 2;
    tga[12..14].copy_from_slice(&(width as u16).to_le_bytes());
    tga[14..16].copy_from_slice(&(height as u16).to_le_bytes());
    tga[16] = 32;
    tga[17] = 0x28; // top-left origin, 8 alpha bits
    for y in 0..height {
        let line = unsafe { std::slice::from_raw_parts(base.add(y * row), width * 4) };
        tga.extend_from_slice(line);
    }
    unsafe { surface.unlock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut()) };
    std::fs::write(out, tga).unwrap();
}
