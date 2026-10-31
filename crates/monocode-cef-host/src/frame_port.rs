//! Frames from the CEF host to the app, one Mach message each: a send right
//! to the IOSurface that holds the frame, with the browser it belongs to and
//! its size. The pixels never leave the GPU. The app checks in a service under
//! a name only it knows and passes the name to the host, which looks it up.
//!
//! Shared by both sides: the host compiles it as a module and the app
//! includes it with `#[path]`.
#![allow(dead_code)]

use mach2::{
    bootstrap::{bootstrap_check_in, bootstrap_look_up, bootstrap_port},
    kern_return::KERN_SUCCESS,
    mach_port::mach_port_deallocate,
    message::{
        MACH_MSG_SUCCESS, MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_MOVE_SEND, MACH_MSGH_BITS, MACH_MSGH_BITS_COMPLEX,
        MACH_RCV_MSG, MACH_RCV_TIMEOUT, MACH_SEND_MSG, MACH_SEND_TIMEOUT, mach_msg,
        mach_msg_body_t, mach_msg_header_t, mach_msg_port_descriptor_t, mach_msg_trailer_t,
    },
    port::{MACH_PORT_NULL, mach_port_t},
    traps::mach_task_self,
};
use std::{ffi::CString, mem::size_of, time::Duration};

const FRAME_ID: i32 = 0x4d43_4546; // "MCEF"

#[repr(C)]
#[derive(Clone, Copy)]
struct FrameMessage {
    header: mach_msg_header_t,
    body: mach_msg_body_t,
    surface: mach_msg_port_descriptor_t,
    browser: u64,
    width: u32,
    height: u32,
}

#[repr(C)]
struct Received {
    message: FrameMessage,
    trailer: mach_msg_trailer_t,
}

/// One frame as received: the caller owns `surface`, a send right, and must
/// deallocate it (see `release`).
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub browser: u64,
    pub surface: mach_port_t,
    pub width: u32,
    pub height: u32,
}

/// Gives back a port right received in a `Frame`.
pub fn release(port: mach_port_t) {
    if port != MACH_PORT_NULL {
        unsafe { mach_port_deallocate(mach_task_self(), port) };
    }
}

/// The app's end: a service the host sends frames to.
pub struct Receiver {
    port: mach_port_t,
    name: String,
}

impl Receiver {
    /// Checks in a service named after this process and `tag`.
    pub fn new(tag: &str) -> Result<Self, String> {
        let name = format!("dev.monocode.cef.{}.{tag}", std::process::id());
        let c_name = CString::new(name.clone()).map_err(|e| e.to_string())?;
        let mut port = MACH_PORT_NULL;
        let result = unsafe { bootstrap_check_in(bootstrap_port, c_name.as_ptr(), &mut port) };
        if result != KERN_SUCCESS {
            return Err(format!("bootstrap_check_in {name} failed: {result}"));
        }
        Ok(Self { port, name })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Waits up to `limit` for the next frame.
    pub fn recv(&self, limit: Duration) -> Option<Frame> {
        let mut received: Received = unsafe { std::mem::zeroed() };
        let result = unsafe {
            mach_msg(
                &mut received.message.header,
                MACH_RCV_MSG | MACH_RCV_TIMEOUT,
                0,
                size_of::<Received>() as u32,
                self.port,
                limit.as_millis().min(u32::MAX as u128) as u32,
                MACH_PORT_NULL,
            )
        };
        if result != MACH_MSG_SUCCESS {
            return None;
        }
        let message = received.message;
        if message.header.msgh_id != FRAME_ID
            || message.header.msgh_bits & MACH_MSGH_BITS_COMPLEX == 0
            || message.body.msgh_descriptor_count != 1
        {
            release(message.surface.name);
            return None;
        }
        Some(Frame {
            browser: message.browser,
            surface: message.surface.name,
            width: message.width,
            height: message.height,
        })
    }
}

/// The host's end.
pub struct Sender {
    port: mach_port_t,
}

impl Sender {
    pub fn connect(name: &str) -> Result<Self, String> {
        let c_name = CString::new(name).map_err(|e| e.to_string())?;
        let mut port = MACH_PORT_NULL;
        let result = unsafe { bootstrap_look_up(bootstrap_port, c_name.as_ptr(), &mut port) };
        if result != KERN_SUCCESS {
            return Err(format!("bootstrap_look_up {name} failed: {result}"));
        }
        Ok(Self { port })
    }

    /// Sends a frame, moving the send right `surface` to the app. A full
    /// queue drops the frame rather than wait.
    pub fn send(&self, browser: u64, surface: mach_port_t, width: u32, height: u32) -> bool {
        let mut message = FrameMessage {
            header: mach_msg_header_t {
                msgh_bits: MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, 0) | MACH_MSGH_BITS_COMPLEX,
                msgh_size: size_of::<FrameMessage>() as u32,
                msgh_remote_port: self.port,
                msgh_local_port: MACH_PORT_NULL,
                msgh_voucher_port: MACH_PORT_NULL,
                msgh_id: FRAME_ID,
            },
            body: mach_msg_body_t {
                msgh_descriptor_count: 1,
            },
            surface: mach_msg_port_descriptor_t::new(surface, MACH_MSG_TYPE_MOVE_SEND),
            browser,
            width,
            height,
        };
        let result = unsafe {
            mach_msg(
                &mut message.header,
                MACH_SEND_MSG | MACH_SEND_TIMEOUT,
                size_of::<FrameMessage>() as u32,
                0,
                MACH_PORT_NULL,
                0,
                MACH_PORT_NULL,
            )
        };
        if result != MACH_MSG_SUCCESS {
            release(surface);
            return false;
        }
        true
    }
}
