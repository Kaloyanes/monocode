//! The two wire formats the device hub serves, kept in one place so a hub
//! update changes one file.
//!
//! iOS (serve-sim): video is an HTTP body of AVCC envelopes, a 4-byte
//! big-endian length then a tag byte and its payload; input is a binary
//! WebSocket of a tag byte and JSON. Android (serve-emu): one WebSocket
//! carries SEMU-framed Annex B H.264 one way and JSON input the other.
use serde::Deserialize;
use serde_json::json;

/// An envelope larger than this is a broken stream, not a frame.
const MAX_ENVELOPE: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chunk {
    /// An `avcC` record: the stream's parameter sets.
    Description(Vec<u8>),
    /// Length-prefixed NAL units of an IDR access unit.
    Keyframe(Vec<u8>),
    /// Length-prefixed NAL units of any other access unit.
    Delta(Vec<u8>),
    /// A JPEG of the screen to show until the first frame decodes.
    Seed(Vec<u8>),
}

/// Splits serve-sim's AVCC body into chunks as bytes arrive.
#[derive(Default)]
pub struct AvccDemuxer {
    buffer: Vec<u8>,
}

impl AvccDemuxer {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Chunk>, String> {
        self.buffer.extend_from_slice(bytes);
        let mut chunks = Vec::new();
        let mut offset = 0;
        while self.buffer.len() - offset >= 4 {
            let length =
                u32::from_be_bytes(self.buffer[offset..offset + 4].try_into().unwrap()) as usize;
            if length > MAX_ENVELOPE {
                return Err(format!("Device stream sent a {length}-byte frame"));
            }
            if self.buffer.len() - offset - 4 < length {
                break;
            }
            if length >= 1 {
                let payload = self.buffer[offset + 5..offset + 4 + length].to_vec();
                match self.buffer[offset + 4] {
                    1 => chunks.push(Chunk::Description(payload)),
                    2 => chunks.push(Chunk::Keyframe(payload)),
                    3 => chunks.push(Chunk::Delta(payload)),
                    4 => chunks.push(Chunk::Seed(payload)),
                    _ => {}
                }
            }
            offset += 4 + length;
        }
        self.buffer.drain(..offset);
        Ok(chunks)
    }
}

const SEMU_MAGIC: u32 = 0x5345_4d55;
const SEMU_HEADER: usize = 16;

/// One video message from serve-emu.
#[derive(Debug, PartialEq, Eq)]
pub struct Semu<'a> {
    /// Annex B H.264.
    pub data: &'a [u8],
    /// From the header; `None` when the hub sent bare H.264.
    pub key: Option<bool>,
    pub pts: Option<u64>,
}

pub fn semu(packet: &[u8]) -> Semu<'_> {
    if packet.len() > SEMU_HEADER
        && u32::from_be_bytes(packet[0..4].try_into().unwrap()) == SEMU_MAGIC
        && packet[4] == 1
    {
        return Semu {
            data: &packet[SEMU_HEADER..],
            key: Some(packet[5] & 1 != 0),
            pts: Some(u64::from_be_bytes(packet[8..16].try_into().unwrap())),
        };
    }
    Semu {
        data: packet,
        key: None,
        pts: None,
    }
}

/// serve-emu tells the viewer its encoder restarted, as after a rotation.
pub fn is_video_session(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .is_ok_and(|value| value["type"] == "video-session")
}

/// H.264 parameter sets, ready for a CoreMedia format description.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParameterSets {
    pub sps: Vec<Vec<u8>>,
    pub pps: Vec<Vec<u8>>,
}

impl ParameterSets {
    pub fn is_complete(&self) -> bool {
        !self.sps.is_empty() && !self.pps.is_empty()
    }
}

/// Reads an `avcC` record, giving each SPS the reorder limits serve-sim's
/// fallback encoder leaves out (see [`with_reorder_limits`]).
pub fn avcc_parameter_sets(description: &[u8]) -> Option<ParameterSets> {
    if description.len() < 7 || description[0] != 1 {
        return None;
    }
    let mut sets = ParameterSets::default();
    let mut position = 6;
    let read = |position: &mut usize| -> Option<Vec<u8>> {
        let length = u16::from_be_bytes(description.get(*position..*position + 2)?.try_into().ok()?)
            as usize;
        *position += 2;
        let unit = description.get(*position..*position + length)?;
        *position += length;
        (length > 0).then(|| unit.to_vec())
    };
    for _ in 0..description[5] & 0x1f {
        sets.sps.push(with_reorder_limits(&read(&mut position)?));
    }
    let count = *description.get(position)?;
    position += 1;
    for _ in 0..count {
        sets.pps.push(read(&mut position)?);
    }
    sets.is_complete().then_some(sets)
}

/// The NAL units of an Annex B byte stream, start codes removed.
pub fn annexb_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            if data[i + 2] == 1 {
                starts.push((i, i + 3));
                i += 3;
                continue;
            }
            if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                starts.push((i, i + 4));
                i += 4;
                continue;
            }
        }
        i += 1;
    }
    let mut units = Vec::with_capacity(starts.len());
    for (index, &(_, begin)) in starts.iter().enumerate() {
        let end = starts.get(index + 1).map_or(data.len(), |&(next, _)| next);
        // A zero before the next start code is trailing padding.
        let mut end = end;
        while end > begin && data[end - 1] == 0 && index + 1 < starts.len() {
            end -= 1;
        }
        if end > begin {
            units.push(&data[begin..end]);
        }
    }
    units
}

/// An Annex B access unit as CoreMedia wants it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AccessUnit {
    /// Length-prefixed slices and SEI; parameter sets and delimiters removed.
    pub avcc: Vec<u8>,
    /// Parameter sets carried in this unit, as a keyframe's are.
    pub sets: ParameterSets,
    pub key: bool,
}

pub fn annexb_access_unit(data: &[u8]) -> AccessUnit {
    let mut unit = AccessUnit::default();
    for nal in annexb_units(data) {
        match nal[0] & 0x1f {
            7 => unit.sets.sps.push(with_reorder_limits(nal)),
            8 => unit.sets.pps.push(nal.to_vec()),
            // Access unit delimiters carry nothing a decoder needs.
            9 => {}
            kind => {
                unit.key |= kind == 5;
                unit.avcc
                    .extend_from_slice(&(nal.len() as u32).to_be_bytes());
                unit.avcc.extend_from_slice(nal);
            }
        }
    }
    unit
}

/// Bits of an SPS, emulation prevention removed.
struct Bits {
    bits: Vec<bool>,
    position: usize,
}

impl Bits {
    fn read(&mut self, count: usize) -> Option<u32> {
        if self.position + count > self.bits.len() || count > 32 {
            return None;
        }
        let mut value = 0u32;
        for _ in 0..count {
            value = (value << 1) | u32::from(self.bits[self.position]);
            self.position += 1;
        }
        Some(value)
    }
    fn golomb(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.read(1)? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        if zeros == 0 {
            return Some(0);
        }
        Some((1u32 << zeros) - 1 + self.read(zeros)?)
    }
}

fn golomb_bits(value: u32, out: &mut Vec<bool>) {
    let coded = u64::from(value) + 1;
    let width = 64 - coded.leading_zeros() as usize;
    out.extend(std::iter::repeat_n(false, width - 1));
    for bit in (0..width).rev() {
        out.push(coded >> bit & 1 == 1);
    }
}

/// serve-sim never reorders frames, but its fallback encoder writes an SPS
/// without VUI, and a decoder then assumes it may and holds frames back. Adds
/// VUI that says no frame is reordered. Only the hub's 8-bit 4:2:0 High
/// profile without scaling lists is rewritten; anything else passes through.
/// Ported from T3 Code's `avcConfig.ts`.
pub fn with_reorder_limits(sps: &[u8]) -> Vec<u8> {
    rewrite_sps(sps).unwrap_or_else(|| sps.to_vec())
}

fn rewrite_sps(sps: &[u8]) -> Option<Vec<u8>> {
    if sps.len() < 4 || sps.len() > 4096 || sps[0] & 0x1f != 7 {
        return None;
    }
    let mut rbsp = Vec::with_capacity(sps.len());
    let mut zeros = 0;
    for (index, &byte) in sps.iter().enumerate().skip(1) {
        if zeros >= 2 && byte == 3 {
            if sps.get(index + 1).is_some_and(|next| *next > 3) {
                return None;
            }
            zeros = 0;
            continue;
        }
        rbsp.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    let mut bits = Bits {
        bits: rbsp
            .iter()
            .flat_map(|byte| (0..8).rev().map(move |bit| byte >> bit & 1 == 1))
            .collect(),
        position: 0,
    };
    if bits.read(8)? != 100 {
        return None;
    }
    bits.read(16)?;
    if bits.golomb()? > 31 || bits.golomb()? != 1 {
        return None;
    }
    if bits.golomb()? != 0 || bits.golomb()? != 0 {
        return None;
    }
    bits.read(1)?;
    if bits.read(1)? != 0 || bits.golomb()? > 12 {
        return None;
    }
    match bits.golomb()? {
        0 => {
            if bits.golomb()? > 12 {
                return None;
            }
        }
        1 => {
            bits.read(1)?;
            bits.golomb()?;
            bits.golomb()?;
            let cycle = bits.golomb()?;
            if cycle > 255 {
                return None;
            }
            for _ in 0..cycle {
                bits.golomb()?;
            }
        }
        2 => {}
        _ => return None,
    }
    let references = bits.golomb()?;
    if references > 16 {
        return None;
    }
    bits.read(1)?;
    if bits.golomb()? > 65535 || bits.golomb()? > 65535 {
        return None;
    }
    if bits.read(1)? == 0 {
        bits.read(1)?;
    }
    bits.read(1)?;
    if bits.read(1)? != 0 {
        for _ in 0..4 {
            bits.golomb()?;
        }
    }
    let vui = bits.position;
    if bits.read(1)? != 0 {
        return None;
    }
    // With no VUI only the stop bit and byte padding may follow.
    let rest = &bits.bits[bits.position..];
    if rest.is_empty() || !rest[0] || rest.len() > 8 || rest[1..].iter().any(|bit| *bit) {
        return None;
    }
    let mut out: Vec<bool> = bits.bits[..vui].to_vec();
    // VUI present, eight absent sections, then bitstream restrictions with
    // motion vectors over picture boundaries allowed.
    out.extend("10000000011".chars().map(|c| c == '1'));
    for value in [2, 1, 16, 16, 0, references] {
        golomb_bits(value, &mut out);
    }
    out.push(true);
    while !out.len().is_multiple_of(8) {
        out.push(false);
    }
    let mut result = vec![sps[0]];
    let mut zeros = 0;
    for byte in out
        .chunks(8)
        .map(|bits| bits.iter().fold(0u8, |acc, bit| acc << 1 | u8::from(*bit)))
    {
        if zeros >= 2 && byte <= 3 {
            result.push(3);
            zeros = 0;
        }
        result.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    Some(result)
}

/// serve-sim's input message tags.
pub mod ios {
    pub const TOUCH: u8 = 0x03;
    pub const BUTTON: u8 = 0x04;
    pub const KEY: u8 = 0x06;
    pub const ORIENTATION: u8 = 0x07;
    /// A foldable's pose, answered by [`HINGE_RESULT`].
    pub const HINGE: u8 = 0x10;
    pub const SCREEN_CONFIG: u8 = 0x82;
    pub const INPUT_ADMITTED: u8 = 0x83;
    pub const HINGE_RESULT: u8 = 0x90;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Begin,
    Move,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Home,
    AppSwitcher,
    Power,
    /// Android only.
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    #[default]
    Portrait,
    LandscapeLeft,
    PortraitUpsideDown,
    LandscapeRight,
}

impl Orientation {
    pub fn name(self) -> &'static str {
        match self {
            Self::Portrait => "portrait",
            Self::LandscapeLeft => "landscape_left",
            Self::PortraitUpsideDown => "portrait_upside_down",
            Self::LandscapeRight => "landscape_right",
        }
    }
    /// A quarter turn, in the order the Simulator's rotate command uses.
    pub fn next(self) -> Self {
        match self {
            Self::Portrait => Self::LandscapeLeft,
            Self::LandscapeLeft => Self::PortraitUpsideDown,
            Self::PortraitUpsideDown => Self::LandscapeRight,
            Self::LandscapeRight => Self::Portrait,
        }
    }
}

/// A foldable's pose, as serve-sim names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
    Closed,
    Book,
    Open,
    Laptop,
    Tent,
}

impl Pose {
    pub const ALL: [Self; 5] = [
        Self::Open,
        Self::Book,
        Self::Laptop,
        Self::Tent,
        Self::Closed,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Book => "book",
            Self::Open => "open",
            Self::Laptop => "laptop",
            Self::Tent => "tent",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|pose| pose.name() == name)
    }
    /// As the Simulator's Device menu words it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Closed => "Fully folded",
            Self::Book => "Partially open",
            Self::Open => "Fully open",
            Self::Laptop => "Laptop",
            Self::Tent => "Tent",
        }
    }
}

fn lenient_pose<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Pose>, D::Error> {
    Ok(Option::<String>::deserialize(d)?.and_then(|name| Pose::from_name(&name)))
}

/// How far the picture turns to stand upright, clockwise as seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Rotation {
    #[default]
    None,
    Clockwise,
    Half,
    CounterClockwise,
}

impl Rotation {
    pub fn quarter_turn(self) -> bool {
        matches!(self, Self::Clockwise | Self::CounterClockwise)
    }
    /// Radians, clockwise as seen.
    pub fn radians(self) -> f64 {
        use std::f64::consts::{FRAC_PI_2, PI};
        match self {
            Self::None => 0.,
            Self::Clockwise => FRAC_PI_2,
            Self::Half => PI,
            Self::CounterClockwise => -FRAC_PI_2,
        }
    }
    /// A `width`×`height` picture's size once turned.
    pub fn turned<T>(self, width: T, height: T) -> (T, T) {
        if self.quarter_turn() {
            (height, width)
        } else {
            (width, height)
        }
    }
}

/// What serve-sim reports about the screen it is streaming.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScreenConfig {
    pub width: f64,
    pub height: f64,
    pub orientation: Orientation,
    #[serde(default)]
    pub input_unavailable: bool,
    /// Which of a foldable's displays is streaming: the iPhone Duo's cover
    /// screen while folded, its inner one otherwise.
    #[serde(default)]
    pub screen_id: Option<u32>,
    #[serde(default)]
    pub supports_hinge_angle: bool,
    #[serde(default)]
    pub hinge_angle: Option<f64>,
    #[serde(default, deserialize_with = "lenient_pose")]
    pub hinge_pose: Option<Pose>,
}

impl ScreenConfig {
    pub fn parse(payload: &[u8]) -> Option<Self> {
        let config: Self = serde_json::from_slice(payload).ok()?;
        (config.width > 0. && config.height > 0.).then_some(config)
    }
    /// serve-sim streams the raw framebuffer. When the interface is turned
    /// against it (upside down, or on its side while the framebuffer is
    /// tall, as an unfolded iPhone Duo always is) the picture is turned to
    /// match. As the hub's own viewer does (`streamGeometry`).
    pub fn rotation(&self) -> Rotation {
        let wide = self.width > self.height;
        match self.orientation {
            Orientation::Portrait => Rotation::None,
            Orientation::PortraitUpsideDown => Rotation::Half,
            Orientation::LandscapeLeft if !wide => Rotation::Clockwise,
            Orientation::LandscapeRight if !wide => Rotation::CounterClockwise,
            Orientation::LandscapeLeft | Orientation::LandscapeRight => Rotation::None,
        }
    }
    /// A point 0–1 across the picture as shown, in the raw framebuffer's
    /// space that serve-sim's input takes.
    pub fn raw_point(&self, x: f64, y: f64) -> (f64, f64) {
        match self.rotation() {
            Rotation::None => (x, y),
            Rotation::Clockwise => (y, 1. - x),
            Rotation::CounterClockwise => (1. - y, x),
            Rotation::Half => (1. - x, 1. - y),
        }
    }
}

/// serve-sim's answer to a [`hinge`] request.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HingeResult {
    pub request_id: u64,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
}

impl HingeResult {
    pub fn parse(payload: &[u8]) -> Option<Self> {
        serde_json::from_slice(payload).ok()
    }
}

/// Folds or opens a foldable. `request_id` must be positive; the answer
/// carries it back.
pub fn hinge(request_id: u64, pose: Pose) -> Message {
    Message::Binary(tagged(
        ios::HINGE,
        json!({"requestId": request_id.max(1), "command": {"control": "pose", "value": pose.name()}}),
    ))
}

/// Opens a foldable's hinge to `degrees`, 0 (folded) to 180 (flat). The
/// hub refuses an angle between 54 and 55, where it can't tell which screen
/// is meant, so one there is taken to 55.
pub fn hinge_angle(request_id: u64, degrees: f64) -> Message {
    let degrees = degrees.clamp(0., 180.);
    let degrees = if degrees > 54. && degrees < 55. {
        55.
    } else {
        degrees.round()
    };
    Message::Binary(tagged(
        ios::HINGE,
        json!({"requestId": request_id.max(1), "command": {"control": "angle", "value": degrees}}),
    ))
}

/// The angle the hub gives each pose, as its `poses` table has it.
impl Pose {
    pub fn angle(self) -> f64 {
        match self {
            Self::Closed => 0.,
            Self::Book | Self::Laptop => 90.,
            Self::Tent => 80.,
            Self::Open => 180.,
        }
    }
}

fn tagged(tag: u8, payload: serde_json::Value) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend_from_slice(payload.to_string().as_bytes());
    out
}

/// One message for the device's input socket.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Binary(Vec<u8>),
    Text(String),
}

/// Encodes input for each platform's socket. Coordinates are 0–1 across the
/// picture as streamed.
pub fn touch(
    platform: super::Platform,
    screen: Option<&ScreenConfig>,
    phase: Phase,
    x: f64,
    y: f64,
) -> Message {
    let (x, y) = (x.clamp(0., 1.), y.clamp(0., 1.));
    match platform {
        super::Platform::Ios => {
            let (x, y) = screen.map_or((x, y), |screen| screen.raw_point(x, y));
            let phase = match phase {
                Phase::Begin => "begin",
                Phase::Move => "move",
                Phase::End => "end",
            };
            Message::Binary(tagged(ios::TOUCH, json!({"type": phase, "x": x, "y": y})))
        }
        super::Platform::Android => {
            let action = match phase {
                Phase::Begin => "down",
                Phase::Move => "move",
                Phase::End => "up",
            };
            Message::Text(json!({"type": "touch", "action": action, "x": x, "y": y}).to_string())
        }
    }
}

pub fn button(platform: super::Platform, button: Button) -> Option<Message> {
    match platform {
        super::Platform::Ios => {
            let name = match button {
                Button::Home => "home",
                Button::AppSwitcher => "app_switcher",
                Button::Power => "lock",
                Button::Back => return None,
            };
            Some(Message::Binary(tagged(
                ios::BUTTON,
                json!({"button": name}),
            )))
        }
        super::Platform::Android => {
            let kind = match button {
                Button::Home => "home",
                Button::AppSwitcher => "recents",
                Button::Power => "power",
                Button::Back => "back",
            };
            Some(Message::Text(json!({"type": kind}).to_string()))
        }
    }
}

pub fn orientation(orientation: Orientation) -> Message {
    Message::Binary(tagged(
        ios::ORIENTATION,
        json!({"orientation": orientation.name()}),
    ))
}

/// Asks serve-emu for a fresh keyframe, as after a decoder error.
pub fn reset_video() -> Message {
    Message::Text(json!({"type": "reset-video", "ack": false}).to_string())
}

/// A key as the app names it (GPUI's lowercase key names).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub command: bool,
}

/// The HID usage of a key on a US keyboard.
pub fn hid_usage(key: &str) -> Option<u8> {
    let usage = match key {
        "enter" => 0x28,
        "escape" => 0x29,
        "backspace" => 0x2a,
        "tab" => 0x2b,
        "space" | " " => 0x2c,
        "-" => 0x2d,
        "=" => 0x2e,
        "[" => 0x2f,
        "]" => 0x30,
        "\\" => 0x31,
        ";" => 0x33,
        "'" => 0x34,
        "`" => 0x35,
        "," => 0x36,
        "." => 0x37,
        "/" => 0x38,
        "delete" => 0x4c,
        "right" => 0x4f,
        "left" => 0x50,
        "down" => 0x51,
        "up" => 0x52,
        "0" => 0x27,
        key if key.len() == 1 => {
            let c = key.as_bytes()[0];
            match c {
                b'a'..=b'z' => 0x04 + (c - b'a'),
                b'1'..=b'9' => 0x1e + (c - b'1'),
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(usage)
}

const HID_SHIFT: u8 = 0xe1;
const HID_CONTROL: u8 = 0xe0;
const HID_ALT: u8 = 0xe2;
const HID_COMMAND: u8 = 0xe3;

/// A key press: modifiers held around the key, as a keyboard sends them. On
/// Android a printable key goes as text and the rest as key codes.
pub fn key_press(
    platform: super::Platform,
    key: &str,
    character: Option<&str>,
    modifiers: Modifiers,
) -> Vec<Message> {
    match platform {
        super::Platform::Ios => {
            let Some(usage) = hid_usage(key) else {
                return Vec::new();
            };
            let held: Vec<u8> = [
                (modifiers.shift, HID_SHIFT),
                (modifiers.control, HID_CONTROL),
                (modifiers.alt, HID_ALT),
                (modifiers.command, HID_COMMAND),
            ]
            .into_iter()
            .filter_map(|(on, usage)| on.then_some(usage))
            .collect();
            let key = |phase: &str, usage: u8| {
                Message::Binary(tagged(ios::KEY, json!({"type": phase, "usage": usage})))
            };
            let mut out: Vec<Message> = held.iter().map(|usage| key("down", *usage)).collect();
            out.push(key("down", usage));
            out.push(key("up", usage));
            out.extend(held.iter().rev().map(|usage| key("up", *usage)));
            out
        }
        super::Platform::Android => {
            let code = match key {
                "escape" => return vec![Message::Text(json!({"type": "back"}).to_string())],
                "up" => Some(19),
                "down" => Some(20),
                "left" => Some(21),
                "right" => Some(22),
                "tab" => Some(61),
                "enter" => Some(66),
                "backspace" => Some(67),
                "delete" => Some(112),
                "home" => Some(122),
                "end" => Some(123),
                "pageup" => Some(92),
                "pagedown" => Some(93),
                _ => None,
            };
            if let Some(code) = code {
                return vec![Message::Text(
                    json!({"type": "key", "keycode": code}).to_string(),
                )];
            }
            match character {
                Some(text) if !text.is_empty() && !modifiers.command && !modifiers.control => {
                    text_messages(text)
                }
                _ => Vec::new(),
            }
        }
    }
}

/// Text for Android, in the chunks serve-emu accepts.
pub fn text_messages(text: &str) -> Vec<Message> {
    const MAX_BYTES: usize = 300;
    const MAX_CHUNKS: usize = 64;
    let mut out = Vec::new();
    let mut chunk = String::new();
    for character in text.chars() {
        if chunk.len() + character.len_utf8() > MAX_BYTES {
            out.push(std::mem::take(&mut chunk));
            if out.len() >= MAX_CHUNKS {
                break;
            }
        }
        chunk.push(character);
    }
    if !chunk.is_empty() && out.len() < MAX_CHUNKS {
        out.push(chunk);
    }
    out.into_iter()
        .map(|text| Message::Text(json!({"type": "text", "text": text}).to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Platform;

    fn envelope(tag: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 1) as u32).to_be_bytes().to_vec();
        out.push(tag);
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn demuxer_waits_for_whole_envelopes_across_reads() {
        let mut stream = envelope(1, b"avcc");
        stream.extend(envelope(2, b"key"));
        stream.extend(envelope(9, b"unknown"));
        stream.extend(envelope(3, b"delta"));
        let mut demuxer = AvccDemuxer::default();
        let mut chunks = Vec::new();
        for piece in stream.chunks(3) {
            chunks.extend(demuxer.push(piece).unwrap());
        }
        assert_eq!(
            chunks,
            [
                Chunk::Description(b"avcc".to_vec()),
                Chunk::Keyframe(b"key".to_vec()),
                Chunk::Delta(b"delta".to_vec()),
            ]
        );
        assert!(demuxer.buffer.is_empty());
    }

    #[test]
    fn demuxer_refuses_absurd_lengths() {
        let mut demuxer = AvccDemuxer::default();
        assert!(demuxer.push(&[0xff, 0xff, 0xff, 0xff]).is_err());
    }

    #[test]
    fn semu_header_is_stripped_and_read() {
        let mut packet = SEMU_MAGIC.to_be_bytes().to_vec();
        packet.extend([1, 1, 0, 0]);
        packet.extend(42u64.to_be_bytes());
        packet.extend([0, 0, 0, 1, 0x65]);
        let parsed = semu(&packet);
        assert_eq!(parsed.data, &[0, 0, 0, 1, 0x65]);
        assert_eq!(parsed.key, Some(true));
        assert_eq!(parsed.pts, Some(42));
        assert_eq!(semu(&[0, 0, 1, 0x41]).key, None);
        assert!(is_video_session(r#"{"type":"video-session","width":1}"#));
        assert!(!is_video_session("{}"));
    }

    #[test]
    fn annexb_units_become_length_prefixed_with_sets_split_out() {
        let stream = [
            0, 0, 0, 1, 0x09, 0xf0, // delimiter
            0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1f, // SPS (baseline: passes through)
            0, 0, 1, 0x68, 0xce, // PPS
            0, 0, 1, 0x65, 0x88, 0x84, 0x00, // IDR slice
        ];
        let unit = annexb_access_unit(&stream);
        assert!(unit.key);
        assert_eq!(unit.sets.sps, [vec![0x67, 0x42, 0x00, 0x1f]]);
        assert_eq!(unit.sets.pps, [vec![0x68, 0xce]]);
        assert_eq!(unit.avcc, [0, 0, 0, 4, 0x65, 0x88, 0x84, 0x00]);
    }

    #[test]
    fn avcc_records_yield_their_parameter_sets() {
        let sps = [0x67, 0x42, 0x00, 0x1f];
        let pps = [0x68, 0xce, 0x3c, 0x80];
        let mut record = vec![1, 0x42, 0, 0x1f, 0xff, 0xe1, 0, 4];
        record.extend(sps);
        record.extend([1, 0, 4]);
        record.extend(pps);
        let sets = avcc_parameter_sets(&record).unwrap();
        assert_eq!(sets.sps, [sps.to_vec()]);
        assert_eq!(sets.pps, [pps.to_vec()]);
        assert!(avcc_parameter_sets(&record[..9]).is_none());
    }

    /// A High-profile SPS without VUI, as serve-sim's fallback encoder writes
    /// it, gains VUI with bitstream restrictions; the result still parses to
    /// the same picture fields and ends in a stop bit.
    #[test]
    fn sps_without_vui_gains_reorder_limits() {
        // profile 100, level 31, sps id 0, chroma 1, bit depths 0, no
        // scaling, log2 frame num 0, poc type 2, 1 ref frame, gaps 0,
        // 1206x2622 in macroblocks, frame mbs only, direct 8x8, no crop,
        // no VUI.
        let mut bits = Vec::new();
        let push = |bits: &mut Vec<bool>, value: u32, width: usize| {
            for bit in (0..width).rev() {
                bits.push(value >> bit & 1 == 1);
            }
        };
        push(&mut bits, 100, 8);
        push(&mut bits, 0, 8);
        push(&mut bits, 31, 8);
        for value in [0, 1, 0, 0] {
            golomb_bits(value, &mut bits);
        }
        bits.push(false); // qpprime bypass
        bits.push(false); // no scaling matrix
        golomb_bits(0, &mut bits); // log2 max frame num - 4
        golomb_bits(2, &mut bits); // poc type
        golomb_bits(1, &mut bits); // max ref frames
        bits.push(false); // gaps
        golomb_bits(75, &mut bits);
        golomb_bits(163, &mut bits);
        bits.push(true); // frame mbs only
        bits.push(true); // direct 8x8
        bits.push(false); // cropping
        bits.push(false); // vui
        bits.push(true); // stop bit
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let mut sps = vec![0x67];
        sps.extend(
            bits.chunks(8)
                .map(|b| b.iter().fold(0u8, |acc, bit| acc << 1 | u8::from(*bit))),
        );
        let repaired = with_reorder_limits(&sps);
        assert_ne!(repaired, sps);
        assert!(repaired.len() > sps.len());
        // A repaired SPS is left alone the second time: it has VUI now.
        assert_eq!(with_reorder_limits(&repaired), repaired);
        // Other profiles pass through untouched.
        let baseline = [0x67, 0x42, 0x00, 0x1f, 0xe9];
        assert_eq!(with_reorder_limits(&baseline), baseline);
    }

    /// The table the hub's viewer (`streamGeometry`) follows: turn when
    /// upside down, or on its side over a tall framebuffer.
    #[test]
    fn rotation_follows_the_hubs_viewer() {
        let config = |width, height, orientation| ScreenConfig {
            width,
            height,
            orientation,
            ..Default::default()
        };
        use Orientation::*;
        for (width, height, orientation, rotation) in [
            (1206., 2622., Portrait, Rotation::None),
            (1206., 2622., LandscapeLeft, Rotation::Clockwise),
            (1206., 2622., LandscapeRight, Rotation::CounterClockwise),
            (1206., 2622., PortraitUpsideDown, Rotation::Half),
            // A framebuffer already wide is shown as it comes.
            (2622., 1206., LandscapeLeft, Rotation::None),
            (2622., 1206., LandscapeRight, Rotation::None),
            (2622., 1206., PortraitUpsideDown, Rotation::Half),
        ] {
            assert_eq!(
                config(width, height, orientation).rotation(),
                rotation,
                "{width}x{height} {orientation:?}"
            );
        }
        assert_eq!(Rotation::Clockwise.turned(2007, 2853), (2853, 2007));
        assert_eq!(Rotation::Half.turned(2007, 2853), (2007, 2853));
    }

    /// A point tapped on the turned picture lands on the raw pixel that the
    /// turn put there.
    #[test]
    fn raw_point_undoes_the_turn_the_picture_was_given() {
        // Where a raw 0–1 point appears once the picture is turned.
        let shown = |rotation: Rotation, (x, y): (f64, f64)| match rotation {
            Rotation::None => (x, y),
            Rotation::Clockwise => (1. - y, x),
            Rotation::CounterClockwise => (y, 1. - x),
            Rotation::Half => (1. - x, 1. - y),
        };
        for orientation in [
            Orientation::Portrait,
            Orientation::LandscapeLeft,
            Orientation::LandscapeRight,
            Orientation::PortraitUpsideDown,
        ] {
            let config = ScreenConfig {
                width: 2007.,
                height: 2853.,
                orientation,
                ..Default::default()
            };
            for raw in [(0.1, 0.2), (0.9, 0.05), (0.5, 0.5), (0., 1.)] {
                let (x, y) = shown(config.rotation(), raw);
                let back = config.raw_point(x, y);
                assert!(
                    (back.0 - raw.0).abs() < 1e-9 && (back.1 - raw.1).abs() < 1e-9,
                    "{orientation:?} {raw:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn foldable_screen_config_and_hinge_messages() {
        // As serve-sim reports an unfolded iPhone Duo.
        let config = ScreenConfig::parse(
            br#"{"width":2007,"height":2853,"orientation":"landscape_left","inputUnavailable":false,
                "screenId":3,"supportsPhysicalOrientation":true,"supportsHingeAngle":true,
                "supportsHingeStateReadback":true,"hingeAngle":180,"hingePose":"open",
                "tableModeAvailable":false,"physicalOrientation":"portrait"}"#,
        )
        .unwrap();
        assert_eq!(config.screen_id, Some(3));
        assert!(config.supports_hinge_angle);
        assert_eq!(config.hinge_pose, Some(Pose::Open));
        assert_eq!(config.rotation(), Rotation::Clockwise);
        // An unknown or missing pose is not a broken config.
        let config = ScreenConfig::parse(
            br#"{"width":10,"height":20,"orientation":"portrait","hingePose":"origami"}"#,
        )
        .unwrap();
        assert_eq!(config.hinge_pose, None);
        assert!(!config.supports_hinge_angle);

        let Message::Binary(bytes) = hinge(7, Pose::Closed) else {
            panic!("hinge requests are binary");
        };
        assert_eq!(bytes[0], ios::HINGE);
        let value: serde_json::Value = serde_json::from_slice(&bytes[1..]).unwrap();
        assert_eq!(
            value,
            json!({"requestId": 7, "command": {"control": "pose", "value": "closed"}})
        );
        for (asked, sent) in [(54.4, 55.), (54., 54.), (120.4, 120.), (-3., 0.), (200., 180.)] {
            let Message::Binary(bytes) = hinge_angle(8, asked) else {
                panic!("hinge requests are binary");
            };
            let value: serde_json::Value = serde_json::from_slice(&bytes[1..]).unwrap();
            assert_eq!(
                value,
                json!({"requestId": 8, "command": {"control": "angle", "value": sent}}),
                "{asked}"
            );
        }
        assert_eq!(
            HingeResult::parse(br#"{"requestId":7,"ok":false,"error":"nope"}"#),
            Some(HingeResult {
                request_id: 7,
                ok: false,
                error: Some("nope".into())
            })
        );
        for pose in Pose::ALL {
            assert_eq!(Pose::from_name(pose.name()), Some(pose));
        }
    }

    #[test]
    fn input_encodes_per_platform() {
        let screen = ScreenConfig {
            width: 1206.,
            height: 2622.,
            orientation: Orientation::LandscapeLeft,
            ..Default::default()
        };
        let Message::Binary(touch_bytes) =
            touch(Platform::Ios, Some(&screen), Phase::Begin, 0.25, 0.5)
        else {
            panic!("iOS input is binary");
        };
        assert_eq!(touch_bytes[0], ios::TOUCH);
        let value: serde_json::Value = serde_json::from_slice(&touch_bytes[1..]).unwrap();
        assert_eq!(value, json!({"type":"begin","x":0.5,"y":0.75}));
        assert_eq!(
            touch(Platform::Android, None, Phase::End, 1.5, 0.),
            Message::Text(r#"{"action":"up","type":"touch","x":1.0,"y":0.0}"#.into())
        );
        assert_eq!(
            button(Platform::Android, Button::AppSwitcher),
            Some(Message::Text(r#"{"type":"recents"}"#.into()))
        );
        assert_eq!(button(Platform::Ios, Button::Back), None);
        let shifted = key_press(
            Platform::Ios,
            "a",
            Some("A"),
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        assert_eq!(shifted.len(), 4);
        assert_eq!(
            key_press(Platform::Android, "a", Some("a"), Modifiers::default()),
            [Message::Text(r#"{"text":"a","type":"text"}"#.into())]
        );
        assert_eq!(text_messages(&"x".repeat(301)).len(), 2);
        assert_eq!(
            ScreenConfig::parse(br#"{"width":10,"height":20,"orientation":"portrait"}"#)
                .unwrap()
                .orientation,
            Orientation::Portrait
        );
        assert!(
            ScreenConfig::parse(br#"{"width":0,"height":20,"orientation":"portrait"}"#).is_none()
        );
    }
}
