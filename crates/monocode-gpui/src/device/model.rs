//! A foldable's 3D model: the iPhone Duo in SceneKit, folding as the
//! simulator's hinge does, with the live stream on whichever of its screens
//! is on.
//!
//! The model is a native `SCNView` placed over the window the way `screen`'s
//! video view is. The video view stays placed beneath it, so the decoder
//! keeps going, and each frame SceneKit draws, its delegate takes the frame
//! the video layer is showing and hands it to the lit screen as a Metal
//! texture. GPUI does nothing per frame here either.
//!
//! Sizes are in millimetres, taken from the simulator's own description of
//! the device: its displays' pixel sizes at 460 dpi and their corner masks
//! (`iPhone Duo.simdevicetype`), and its buttons from the `phone15` chrome.
use super::screen::{Renderer, Screen};
use crate::draft::{Overlay, Painted, mark_painted, track_overlay};
use gpui::{Window, canvas, prelude::*};
use monocode_core::device::protocol::Pose;
use objc2::{
    AnyThread, DefinedClass, Encode, Encoding, MainThreadMarker, RefEncode, class, define_class,
    msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject},
};
use objc2_app_kit::{NSView, NSWindowOrderingMode};
use objc2_core_foundation::CFRetained;
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
};

// ---------------------------------------------------------------------------
// The device, in millimetres.

/// One half's outline. The halves meet at the hinge, x = 0.
const HALF_W: f64 = 81.0;
const HALF_H: f64 = 115.0;
/// Each half's thickness; folded, the device is twice this.
const HALF_T: f64 = 4.6;
/// The gap each half keeps from the hinge's axis.
const GAP: f64 = 0.15;
/// The rounded edge all round each half.
const EDGE: f64 = 0.9;
/// The outline's corners away from the hinge and beside it, as the length of
/// edge each continuous curve takes.
const BODY_CORNER: f64 = 14.4;
const HINGE_CORNER: f64 = 2.2;

/// The inner display: 2007×2853 px at 460 dpi, folding across its long
/// side, so it lies 157.5 mm across the open device and 110.8 mm tall. Its
/// mask rounds its four outer corners over 11.3 mm of edge.
const INNER_W: f64 = 157.5;
const INNER_H: f64 = 110.8;
const INNER_CORNER: f64 = 11.33;

/// The cover display on the back of the left half: 1398×2034 px, 77.2 by
/// 112.3 mm. Its mask rounds the corners away from the hinge over 12.5 mm and
/// those beside it over 1.7 mm.
const COVER_W: f64 = 77.2;
const COVER_H: f64 = 112.3;
const COVER_CORNER: f64 = 12.53;
const COVER_HINGE_CORNER: f64 = 1.7;
/// The cover's border on the hinge side.
const COVER_INSET: f64 = 2.0;

/// The screen ids serve-sim reports for each display.
const COVER_ID: u32 = 1;
const INNER_ID: u32 = 3;

// ---------------------------------------------------------------------------
// Outlines.

/// Apple's continuous corner, as the simulator's display masks draw it: three
/// cubics from `extent` along one edge to `extent` along the other, as
/// (along the incoming edge, along the outgoing edge) per unit of extent.
const CORNER: [[f64; 2]; 10] = [
    [1., 0.],
    [0.6523, 0.],
    [0.5262, 0.0362],
    [0.3991, 0.1042],
    [0.272, 0.1722],
    [0.1722, 0.272],
    [0.1042, 0.3991],
    [0.0362, 0.5262],
    [0., 0.6523],
    [0., 1.],
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Segment {
    Line([f64; 2]),
    Cubic([f64; 2], [f64; 2], [f64; 2]),
}

/// A rect from `min` to `max` (y up) with continuous corners, each corner's
/// extent given clockwise from the top left. Starts at the top edge.
fn outline(min: [f64; 2], max: [f64; 2], corners: [f64; 4]) -> ([f64; 2], Vec<Segment>) {
    let [tl, tr, br, bl] = corners;
    // Each corner's point, the way back along the edge coming in, and the way
    // along the edge going out.
    let turns = [
        ([max[0], max[1]], [-1., 0.], [0., -1.], tr),
        ([max[0], min[1]], [0., 1.], [-1., 0.], br),
        ([min[0], min[1]], [1., 0.], [0., 1.], bl),
        ([min[0], max[1]], [0., -1.], [1., 0.], tl),
    ];
    let start = [min[0] + tl, max[1]];
    let mut segments = Vec::new();
    for (corner, back, out, extent) in turns {
        let at = |p: [f64; 2]| {
            [
                corner[0] + (p[0] * back[0] + p[1] * out[0]) * extent,
                corner[1] + (p[0] * back[1] + p[1] * out[1]) * extent,
            ]
        };
        segments.push(Segment::Line(at(CORNER[0])));
        if extent > 0. {
            for c in CORNER[1..].chunks(3) {
                segments.push(Segment::Cubic(at(c[0]), at(c[1]), at(c[2])));
            }
        }
    }
    (start, segments)
}

/// The outline as points, each cubic in `steps` pieces.
fn flatten(start: [f64; 2], segments: &[Segment], steps: usize) -> Vec<[f64; 2]> {
    let mut points = vec![start];
    let mut last = start;
    for segment in segments {
        match *segment {
            Segment::Line(p) => {
                points.push(p);
                last = p;
            }
            Segment::Cubic(c1, c2, p) => {
                for step in 1..=steps {
                    let t = step as f64 / steps as f64;
                    let u = 1. - t;
                    let w = [u * u * u, 3. * u * u * t, 3. * u * t * t, t * t * t];
                    points.push([
                        w[0] * last[0] + w[1] * c1[0] + w[2] * c2[0] + w[3] * p[0],
                        w[0] * last[1] + w[1] * c1[1] + w[2] * c2[1] + w[3] * p[1],
                    ]);
                }
                last = p;
            }
        }
    }
    points.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    points
}

// ---------------------------------------------------------------------------
// Poses.

/// A unit quaternion, x y z w.
type Quat = [f64; 4];

/// The rotation whose columns are where the model's x, y and z axes go.
fn basis(x: [f64; 3], y: [f64; 3], z: [f64; 3]) -> Quat {
    let m = [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]];
    let trace = m[0][0] + m[1][1] + m[2][2];
    let q = if trace > 0. {
        let s = (trace + 1.).sqrt() * 2.;
        [
            (m[2][1] - m[1][2]) / s,
            (m[0][2] - m[2][0]) / s,
            (m[1][0] - m[0][1]) / s,
            s / 4.,
        ]
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1. + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.;
        [
            s / 4.,
            (m[0][1] + m[1][0]) / s,
            (m[0][2] + m[2][0]) / s,
            (m[2][1] - m[1][2]) / s,
        ]
    } else if m[1][1] > m[2][2] {
        let s = (1. + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.;
        [
            (m[0][1] + m[1][0]) / s,
            s / 4.,
            (m[1][2] + m[2][1]) / s,
            (m[0][2] - m[2][0]) / s,
        ]
    } else {
        let s = (1. + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.;
        [
            (m[0][2] + m[2][0]) / s,
            (m[1][2] + m[2][1]) / s,
            s / 4.,
            (m[1][0] - m[0][1]) / s,
        ]
    };
    let n = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    q.map(|v| v / n)
}

fn rotate(q: Quat, v: [f64; 3]) -> [f64; 3] {
    let [x, y, z, w] = q;
    // v + 2w(q×v) + 2q×(q×v)
    let c = [
        y * v[2] - z * v[1],
        z * v[0] - x * v[2],
        x * v[1] - y * v[0],
    ];
    let cc = [
        y * c[2] - z * c[1],
        z * c[0] - x * c[2],
        x * c[1] - y * c[0],
    ];
    [
        v[0] + 2. * (w * c[0] + cc[0]),
        v[1] + 2. * (w * c[1] + cc[1]),
        v[2] + 2. * (w * c[2] + cc[2]),
    ]
}

/// How the device is held in each pose, with the world's y up and the
/// viewer toward +z. The model's hinge runs along its y, the inner display
/// faces +z when flat, and the cover half is the one toward -x.
fn holding(pose: Pose) -> Quat {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    match pose {
        // Flat on the table, the inner display up, its top away.
        Pose::Open => basis([1., 0., 0.], [0., 0., -1.], [0., 1., 0.]),
        // Standing like an open book, facing the viewer.
        Pose::Book => basis([1., 0., 0.], [0., 1., 0.], [0., 0., 1.]),
        // The hinge across, the right half on the table, the left standing.
        Pose::Laptop => basis([0., -s, s], [1., 0., 0.], [0., s, s]),
        // The hinge up, the halves as legs, the cover toward the viewer.
        Pose::Tent => basis([0., 0., -1.], [1., 0., 0.], [0., -1., 0.]),
        // Folded on the table, the cover up and reading upright.
        Pose::Closed => basis([0., -1., 0.], [0., 0., -1.], [1., 0., 0.]),
    }
}

/// How a hinge angle with no pose is held: as the pose it looks most like.
fn nearest(angle: f64) -> Pose {
    if angle <= 54. {
        Pose::Closed
    } else if angle >= 150. {
        Pose::Open
    } else {
        Pose::Book
    }
}

/// Where everything goes for a hinge `angle` held as `pose`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    /// Each half's turn about the hinge; the left half turns by +fold.
    fold: f64,
    holding: Quat,
    /// Moves the held device so it rests on the floor, centred.
    offset: [f64; 3],
    /// The hinge's barrel, which shows more as the device folds.
    spine_radius: f64,
    spine_z: f64,
}

fn layout(angle: f64, pose: Pose) -> Layout {
    let angle = angle.clamp(0., 180.);
    let fold = ((180. - angle) / 2.).to_radians();
    let holding = holding(pose);
    let mut min = [f64::MAX; 3];
    let mut max = [f64::MIN; 3];
    for side in [-1., 1.] {
        // A half turned about y by `side * -fold`: the left by +fold.
        let a = -side * fold;
        let (sa, ca) = (a.sin(), a.cos());
        for x in [side * GAP, side * (GAP + HALF_W)] {
            for y in [-HALF_H / 2., HALF_H / 2.] {
                for z in [0., -HALF_T] {
                    let turned = [x * ca + z * sa, y, -x * sa + z * ca];
                    let p = rotate(holding, turned);
                    for i in 0..3 {
                        min[i] = min[i].min(p[i]);
                        max[i] = max[i].max(p[i]);
                    }
                }
            }
        }
    }
    let k = fold / std::f64::consts::FRAC_PI_2;
    Layout {
        fold,
        holding,
        offset: [
            -(min[0] + max[0]) / 2.,
            -min[1] + 0.2,
            -(min[2] + max[2]) / 2.,
        ],
        spine_radius: HALF_T / 2. + HALF_T / 2. * k,
        spine_z: -HALF_T / 2. + HALF_T / 2. * k,
    }
}

// ---------------------------------------------------------------------------
// SceneKit.

#[link(name = "SceneKit", kind = "framework")]
unsafe extern "C" {
    static SCNLightingModelPhysicallyBased: &'static NSString;
    static SCNLightingModelShadowOnly: &'static NSString;
    static SCNLightTypeDirectional: &'static NSString;
    static SCNLightTypeAmbient: &'static NSString;
}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {
    static kCAMediaTimingFunctionEaseInEaseOut: &'static NSString;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVMetalTextureCacheCreate(
        allocator: *const c_void,
        cache_attributes: *const c_void,
        metal_device: *mut AnyObject,
        texture_attributes: *const c_void,
        cache_out: *mut *mut c_void,
    ) -> i32;
    fn CVMetalTextureCacheCreateTextureFromImage(
        allocator: *const c_void,
        cache: *mut c_void,
        source: *const CVPixelBuffer,
        texture_attributes: *const c_void,
        pixel_format: usize,
        width: usize,
        height: usize,
        plane: usize,
        texture_out: *mut *mut c_void,
    ) -> i32;
    fn CVMetalTextureGetTexture(texture: *mut c_void) -> *mut AnyObject;
    fn CVMetalTextureCacheFlush(cache: *mut c_void, options: u64);
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTPixelTransferSessionCreate(allocator: *const c_void, session: *mut *mut c_void) -> i32;
    fn VTPixelTransferSessionTransferImage(
        session: *mut c_void,
        source: *const CVPixelBuffer,
        destination: *const CVPixelBuffer,
    ) -> i32;
    fn VTPixelTransferSessionInvalidate(session: *mut c_void);
}

unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// `MTLPixelFormatBGRA8Unorm_sRGB`: the stream is sRGB, and SceneKit lights
/// in linear space.
const BGRA8_SRGB: usize = 81;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Vec3 {
    x: f64,
    y: f64,
    z: f64,
}
unsafe impl Encode for Vec3 {
    const ENCODING: Encoding = Encoding::Struct("SCNVector3", &[f64::ENCODING; 3]);
}
unsafe impl RefEncode for Vec3 {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Vec4 {
    x: f64,
    y: f64,
    z: f64,
    w: f64,
}
unsafe impl Encode for Vec4 {
    const ENCODING: Encoding = Encoding::Struct("SCNVector4", &[f64::ENCODING; 4]);
}

fn v3(x: f64, y: f64, z: f64) -> Vec3 {
    Vec3 { x, y, z }
}

type Id = Retained<AnyObject>;

fn number(value: f64) -> Id {
    unsafe { msg_send![class!(NSNumber), numberWithDouble: value] }
}

fn color(r: f64, g: f64, b: f64, a: f64) -> Id {
    unsafe { msg_send![class!(NSColor), colorWithSRGBRed: r, green: g, blue: b, alpha: a] }
}

fn property(material: &AnyObject, name: &str) -> Id {
    let name = NSString::from_str(name);
    unsafe { msg_send![material, valueForKey: &*name] }
}

fn set_contents(material: &AnyObject, name: &str, contents: &AnyObject) {
    let property = property(material, name);
    let _: () = unsafe { msg_send![&*property, setContents: contents] };
}

/// A physically based material.
fn pbr(base: Id, metalness: f64, roughness: f64) -> Id {
    let material: Id = unsafe { msg_send![class!(SCNMaterial), material] };
    let _: () =
        unsafe { msg_send![&*material, setLightingModelName: SCNLightingModelPhysicallyBased] };
    set_contents(&material, "diffuse", &base);
    set_contents(&material, "metalness", &number(metalness));
    set_contents(&material, "roughness", &number(roughness));
    material
}

fn node(geometry: Option<&AnyObject>) -> Id {
    match geometry {
        Some(geometry) => unsafe { msg_send![class!(SCNNode), nodeWithGeometry: geometry] },
        None => unsafe { msg_send![class!(SCNNode), node] },
    }
}

fn add(parent: &AnyObject, child: &AnyObject) {
    let _: () = unsafe { msg_send![parent, addChildNode: child] };
}

fn place_node(node: &AnyObject, at: Vec3) {
    let _: () = unsafe { msg_send![node, setPosition: at] };
}

fn set_materials(geometry: &AnyObject, materials: &[&AnyObject]) {
    let materials = NSArray::from_slice(materials);
    let _: () = unsafe { msg_send![geometry, setMaterials: &*materials] };
}

fn rounded_box(w: f64, h: f64, l: f64, chamfer: f64, material: &AnyObject) -> Id {
    let geometry: Id = unsafe {
        msg_send![class!(SCNBox), boxWithWidth: w, height: h, length: l, chamferRadius: chamfer]
    };
    set_materials(&geometry, &[material]);
    node(Some(&geometry))
}

/// A disc facing +z: a cylinder `depth` deep.
fn disc(radius: f64, depth: f64, material: &AnyObject) -> Id {
    let geometry: Id =
        unsafe { msg_send![class!(SCNCylinder), cylinderWithRadius: radius, height: depth] };
    let _: () = unsafe { msg_send![&*geometry, setRadialSegmentCount: 64isize] };
    set_materials(&geometry, &[material]);
    let disc = node(Some(&geometry));
    let _: () =
        unsafe { msg_send![&*disc, setEulerAngles: v3(std::f64::consts::FRAC_PI_2, 0., 0.)] };
    disc
}

/// A half's body: its outline extruded, edges rounded, its inner face at
/// z = 0 and its back at -HALF_T.
fn body(side: f64, front: &AnyObject, back: &AnyObject, frame: &AnyObject) -> Id {
    let (near, far) = (GAP, GAP + HALF_W);
    let (min, max, corners) = if side < 0. {
        (
            [-far, -HALF_H / 2.],
            [-near, HALF_H / 2.],
            [BODY_CORNER, HINGE_CORNER, HINGE_CORNER, BODY_CORNER],
        )
    } else {
        (
            [near, -HALF_H / 2.],
            [far, HALF_H / 2.],
            [HINGE_CORNER, BODY_CORNER, BODY_CORNER, HINGE_CORNER],
        )
    };
    let (start, segments) = outline(min, max, corners);
    let path: Id = unsafe { msg_send![class!(NSBezierPath), bezierPath] };
    let point = |p: [f64; 2]| NSPoint::new(p[0], p[1]);
    unsafe {
        let _: () = msg_send![&*path, moveToPoint: point(start)];
        for segment in &segments {
            match *segment {
                Segment::Line(p) => {
                    let _: () = msg_send![&*path, lineToPoint: point(p)];
                }
                Segment::Cubic(c1, c2, p) => {
                    let _: () = msg_send![&*path, curveToPoint: point(p), controlPoint1: point(c1), controlPoint2: point(c2)];
                }
            }
        }
        let _: () = msg_send![&*path, closePath];
        let _: () = msg_send![&*path, setFlatness: 0.08f64];
    }
    let shape: Id =
        unsafe { msg_send![class!(SCNShape), shapeWithPath: &*path, extrusionDepth: HALF_T] };
    unsafe {
        let _: () = msg_send![&*shape, setChamferRadius: EDGE];
        let _: () = msg_send![&*shape, setChamferMode: 0isize];
    }
    // Front, back, sides, front chamfer, back chamfer.
    set_materials(&shape, &[front, back, frame, frame, frame]);
    let body = node(Some(&shape));
    place_node(&body, v3(0., 0., -HALF_T / 2.));
    let _: () = unsafe { msg_send![&*body, setCastsShadow: true] };
    body
}

/// A flat screen facing +z, from an outline, with each point's place in the
/// picture from `uv`.
fn screen_geometry(points: &[[f64; 2]], uv: impl Fn([f64; 2]) -> [f64; 2]) -> Id {
    let n = points.len();
    let centre = points.iter().fold([0., 0.], |a, p| {
        [a[0] + p[0] / n as f64, a[1] + p[1] / n as f64]
    });
    let mut vertices = vec![v3(centre[0], centre[1], 0.)];
    let mut coords = vec![{
        let [u, v] = uv(centre);
        NSPoint::new(u, v)
    }];
    for p in points {
        vertices.push(v3(p[0], p[1], 0.));
        let [u, v] = uv(*p);
        coords.push(NSPoint::new(u, v));
    }
    let normals = vec![v3(0., 0., 1.); vertices.len()];
    let mut indices: Vec<u16> = Vec::with_capacity(n * 3);
    for i in 0..n {
        let a = 1 + i as u16;
        let b = 1 + ((i + 1) % n) as u16;
        // Counterclockwise seen from +z; the outline runs clockwise.
        indices.extend_from_slice(&[0, b, a]);
    }
    unsafe {
        let positions: Id = msg_send![class!(SCNGeometrySource), geometrySourceWithVertices: vertices.as_ptr(), count: vertices.len() as isize];
        let normals: Id = msg_send![class!(SCNGeometrySource), geometrySourceWithNormals: normals.as_ptr(), count: normals.len() as isize];
        let coords: Id = msg_send![class!(SCNGeometrySource), geometrySourceWithTextureCoordinates: coords.as_ptr(), count: coords.len() as isize];
        let bytes = objc2_foundation::NSData::with_bytes(std::slice::from_raw_parts(
            indices.as_ptr().cast::<u8>(),
            indices.len() * 2,
        ));
        let element: Id = msg_send![class!(SCNGeometryElement), geometryElementWithData: &*bytes, primitiveType: 0isize, primitiveCount: n as isize, bytesPerIndex: 2isize];
        let sources = NSArray::from_slice(&[&*positions, &*normals, &*coords]);
        let elements = NSArray::from_slice(&[&*element]);
        msg_send![class!(SCNGeometry), geometryWithSources: &*sources, elements: &*elements]
    }
}

/// A screen's glass: black while it is off, the stream's frame lit up while
/// it is on.
fn glass() -> Id {
    let material = pbr(color(0.01, 0.01, 0.012, 1.), 0., 0.06);
    set_contents(&material, "emission", &color(0., 0., 0., 1.));
    material
}

/// An environment to light and reflect in: a bright ceiling, a darker floor
/// and three soft boxes, as an equirectangular BMP.
fn environment() -> Option<Id> {
    const W: usize = 256;
    const H: usize = 128;
    let mut pixels = vec![0u8; W * H * 4];
    for row in 0..H {
        // BMP rows run bottom up.
        let elevation = (row as f64 / (H - 1) as f64 - 0.5) * std::f64::consts::PI;
        for col in 0..W {
            let azimuth = (col as f64 / W as f64) * std::f64::consts::TAU;
            let mut v = if elevation > 0. {
                0.62 + 0.3 * elevation.sin()
            } else {
                0.42 + 0.2 * elevation.sin()
            };
            for (az, el, w, h) in [
                (0.6, 0.75, 0.55, 0.22),
                (2.9, 0.5, 0.35, 0.3),
                (4.6, 0.95, 0.9, 0.18),
            ] {
                let d = ((azimuth - az + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                    - std::f64::consts::PI)
                    .abs();
                if d < w && (elevation - el).abs() < h {
                    v = 1.;
                }
            }
            let byte = (v.clamp(0., 1.) * 255.) as u8;
            let i = (row * W + col) * 4;
            pixels[i..i + 4].copy_from_slice(&[byte, byte, byte, 255]);
        }
    }
    let mut bmp = Vec::with_capacity(54 + pixels.len());
    let size = (54 + pixels.len()) as u32;
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&size.to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(W as i32).to_le_bytes());
    bmp.extend_from_slice(&(H as i32).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 16]);
    bmp.extend_from_slice(&pixels);
    let data = objc2_foundation::NSData::with_bytes(&bmp);
    let alloc: objc2::rc::Allocated<AnyObject> = unsafe { msg_send![class!(NSImage), alloc] };
    unsafe { msg_send![alloc, initWithData: &*data] }
}

/// The nodes that move and the materials the stream lights.
struct Scene {
    scene: Id,
    camera: Id,
    holder: Id,
    left: Id,
    right: Id,
    spine: Id,
    inner: Id,
    cover: Id,
}

fn build_scene() -> Scene {
    let scene: Id = unsafe { msg_send![class!(SCNScene), scene] };
    let root: Id = unsafe { msg_send![&*scene, rootNode] };
    if let Some(environment) = environment() {
        let lighting: Id = unsafe { msg_send![&*scene, lightingEnvironment] };
        unsafe {
            let _: () = msg_send![&*lighting, setContents: &*environment];
            let _: () = msg_send![&*lighting, setIntensity: 1.25f64];
        }
    }

    // Titanium in a warm grey, a frosted back, black glass round the screens.
    let frame = pbr(color(0.79, 0.77, 0.74, 1.), 1., 0.3);
    let back = pbr(color(0.86, 0.84, 0.8, 1.), 0.05, 0.5);
    let bezel = pbr(color(0.012, 0.012, 0.014, 1.), 0., 0.08);
    let lens = pbr(color(0.03, 0.035, 0.045, 1.), 0.3, 0.06);
    let ring = pbr(color(0.72, 0.7, 0.67, 1.), 1., 0.18);
    let flash = pbr(color(0.95, 0.92, 0.8, 1.), 0., 0.3);

    let inner = glass();
    let cover = glass();

    let half = |side: f64| -> Id {
        let pivot = node(None);
        add(&pivot, &body(side, &bezel, &back, &frame));
        // This half of the inner display, from the hinge out.
        let (x0, x1) = if side < 0. {
            (-INNER_W / 2., -GAP)
        } else {
            (GAP, INNER_W / 2.)
        };
        let (y0, y1) = (-INNER_H / 2., INNER_H / 2.);
        let corners = if side < 0. {
            [INNER_CORNER, 0., 0., INNER_CORNER]
        } else {
            [0., INNER_CORNER, INNER_CORNER, 0.]
        };
        let (start, segments) = outline([x0, y0], [x1, y1], corners);
        let points = flatten(start, &segments, 8);
        // The framebuffer is tall, its hinge across its middle: its top
        // half is the left half, and it runs left to right up the model.
        let geometry = screen_geometry(&points, |[x, y]| {
            [(y - y0) / INNER_H, (x + INNER_W / 2.) / INNER_W]
        });
        set_materials(&geometry, &[&*inner]);
        let screen = node(Some(&geometry));
        place_node(&screen, v3(0., 0., 0.03));
        add(&pivot, &screen);
        if side < 0. {
            // The cover display, seen from behind the left half: x runs from
            // the hinge out, the framebuffer's way too.
            let (cx0, cx1) = (GAP + COVER_INSET, GAP + COVER_INSET + COVER_W);
            let (cy0, cy1) = (-COVER_H / 2., COVER_H / 2.);
            let (start, segments) = outline(
                [cx0, cy0],
                [cx1, cy1],
                [
                    COVER_HINGE_CORNER,
                    COVER_CORNER,
                    COVER_CORNER,
                    COVER_HINGE_CORNER,
                ],
            );
            let points = flatten(start, &segments, 8);
            let geometry =
                screen_geometry(&points, |[x, y]| [(x - cx0) / COVER_W, (cy1 - y) / COVER_H]);
            set_materials(&geometry, &[&*cover]);
            let screen = node(Some(&geometry));
            place_node(&screen, v3(0., 0., -HALF_T - 0.03));
            let _: () =
                unsafe { msg_send![&*screen, setEulerAngles: v3(0., std::f64::consts::PI, 0.)] };
            add(&pivot, &screen);
            // Volume on the top edge, power on the outer one, where the
            // simulator's chrome puts them.
            for from_hinge in [40.6, 53.8] {
                let button = rounded_box(10.9, 1.2, 1.7, 0.5, &frame);
                place_node(&button, v3(-from_hinge, HALF_H / 2. + 0.25, -HALF_T / 2.));
                add(&pivot, &button);
            }
            let power = rounded_box(1.2, 18.7, 1.7, 0.5, &frame);
            place_node(
                &power,
                v3(-(GAP + HALF_W) - 0.25, HALF_H / 2. - 41., -HALF_T / 2.),
            );
            add(&pivot, &power);
        } else {
            // The cameras, on the back of the right half by its outer top
            // corner.
            let (bx, by) = (GAP + HALF_W - 13., HALF_H / 2. - 24.);
            let plateau = rounded_box(17., 38., 1.4, 0.65, &frame);
            place_node(&plateau, v3(bx, by, -HALF_T - 0.55));
            let _: () = unsafe { msg_send![&*plateau, setCastsShadow: true] };
            add(&pivot, &plateau);
            for dy in [8.6, -8.6] {
                let outer = disc(5.6, 1.3, &ring);
                place_node(&outer, v3(bx, by + dy, -HALF_T - 1.45));
                add(&pivot, &outer);
                let glass = disc(4.3, 1.4, &lens);
                place_node(&glass, v3(bx, by + dy, -HALF_T - 1.55));
                add(&pivot, &glass);
            }
            let light = disc(1.5, 0.4, &flash);
            place_node(&light, v3(bx - 4.8, by, -HALF_T - 1.3));
            add(&pivot, &light);
        }
        pivot
    };
    let left = half(-1.);
    let right = half(1.);

    let barrel: Id = unsafe {
        msg_send![class!(SCNCylinder), cylinderWithRadius: 1f64, height: HALF_H - 2. * HINGE_CORNER]
    };
    unsafe {
        let _: () = msg_send![&*barrel, setRadialSegmentCount: 48isize];
    }
    set_materials(&barrel, &[&*frame]);
    let spine = node(Some(&barrel));
    let _: () = unsafe { msg_send![&*spine, setCastsShadow: true] };

    let device = node(None);
    add(&device, &left);
    add(&device, &right);
    add(&device, &spine);
    let holder = node(None);
    add(&holder, &device);
    add(&root, &holder);

    // A floor that only takes the device's shadow.
    let floor: Id = unsafe { msg_send![class!(SCNFloor), floor] };
    unsafe {
        let _: () = msg_send![&*floor, setReflectivity: 0f64];
    }
    let shadow: Id = unsafe { msg_send![class!(SCNMaterial), material] };
    unsafe {
        let _: () = msg_send![&*shadow, setLightingModelName: SCNLightingModelShadowOnly];
    }
    set_materials(&floor, &[&*shadow]);
    add(&root, &node(Some(&floor)));

    let sun: Id = unsafe { msg_send![class!(SCNLight), light] };
    unsafe {
        let _: () = msg_send![&*sun, setType: SCNLightTypeDirectional];
        let _: () = msg_send![&*sun, setIntensity: 700f64];
        let _: () = msg_send![&*sun, setCastsShadow: true];
        let _: () = msg_send![&*sun, setShadowMode: 1isize];
        let _: () = msg_send![&*sun, setShadowRadius: 9f64];
        let _: () = msg_send![&*sun, setShadowSampleCount: 16usize];
        let _: () = msg_send![&*sun, setShadowColor: &*color(0., 0., 0., 0.28)];
        let _: () = msg_send![&*sun, setShadowMapSize: NSSize::new(2048., 2048.)];
        let _: () = msg_send![&*sun, setOrthographicScale: 170f64];
        let _: () = msg_send![&*sun, setZNear: 10f64];
        let _: () = msg_send![&*sun, setZFar: 900f64];
        let _: () = msg_send![&*sun, setAutomaticallyAdjustsShadowProjection: false];
    }
    let sun_node = node(None);
    unsafe {
        let _: () = msg_send![&*sun_node, setLight: &*sun];
    }
    place_node(&sun_node, v3(70., 320., 160.));
    let _: () = unsafe { msg_send![&*sun_node, lookAt: v3(0., 0., 0.)] };
    add(&root, &sun_node);
    let fill: Id = unsafe { msg_send![class!(SCNLight), light] };
    unsafe {
        let _: () = msg_send![&*fill, setType: SCNLightTypeAmbient];
        let _: () = msg_send![&*fill, setIntensity: 120f64];
    }
    let fill_node = node(None);
    unsafe {
        let _: () = msg_send![&*fill_node, setLight: &*fill];
    }
    add(&root, &fill_node);

    let lens: Id = unsafe { msg_send![class!(SCNCamera), camera] };
    unsafe {
        let _: () = msg_send![&*lens, setFieldOfView: 30f64];
        let _: () = msg_send![&*lens, setZNear: 5f64];
        let _: () = msg_send![&*lens, setZFar: 4000f64];
    }
    let camera = node(None);
    unsafe {
        let _: () = msg_send![&*camera, setCamera: &*lens];
    }
    place_node(&camera, v3(40., 240., 360.));
    let _: () = unsafe { msg_send![&*camera, lookAt: v3(0., 42., 0.)] };
    add(&root, &camera);

    Scene {
        scene,
        camera,
        holder,
        left,
        right,
        spine,
        inner,
        cover,
    }
}

/// Moves the model to `to`, over `seconds`.
fn apply(scene: &Scene, to: &Layout, seconds: f64) {
    unsafe {
        let _: () = msg_send![class!(SCNTransaction), begin];
        let _: () = msg_send![class!(SCNTransaction), setAnimationDuration: seconds];
        let timing: Id = msg_send![class!(CAMediaTimingFunction), functionWithName: kCAMediaTimingFunctionEaseInEaseOut];
        let _: () = msg_send![class!(SCNTransaction), setAnimationTimingFunction: &*timing];
        let _: () = msg_send![&*scene.left, setEulerAngles: v3(0., to.fold, 0.)];
        let _: () = msg_send![&*scene.right, setEulerAngles: v3(0., -to.fold, 0.)];
        let _: () = msg_send![&*scene.spine, setScale: v3(to.spine_radius, 1., to.spine_radius)];
        place_node(&scene.spine, v3(0., 0., to.spine_z));
        let [x, y, z, w] = to.holding;
        let _: () = msg_send![&*scene.holder, setOrientation: Vec4 { x, y, z, w }];
        place_node(&scene.holder, v3(to.offset[0], to.offset[1], to.offset[2]));
        let _: () = msg_send![class!(SCNTransaction), commit];
    }
}

// ---------------------------------------------------------------------------
// The stream on the lit screen.

/// What the render thread needs, shared with the main thread.
struct Feed {
    renderer: Arc<Renderer>,
    /// The screen id streaming, or 0 while neither is lit.
    lit: AtomicU8,
    state: Mutex<FeedState>,
}

struct FeedState {
    inner: Shared,
    cover: Shared,
    cache: *mut c_void,
    transfer: *mut c_void,
    /// The frame last shown, by address.
    last: usize,
    /// Converted frames, reused in turn while the GPU may still read the
    /// one before.
    buffers: Vec<CFRetained<CVPixelBuffer>>,
    next: usize,
    /// Textures in use, kept until replaced.
    textures: Vec<*mut c_void>,
    /// Which screen has the picture, so the other can go dark.
    showing: u8,
}

/// An Objective-C object the render thread may use: SceneKit takes material
/// changes from its delegate's callbacks.
struct Shared(Id);
unsafe impl Send for FeedState {}

impl Drop for FeedState {
    fn drop(&mut self) {
        unsafe {
            for texture in self.textures.drain(..) {
                CFRelease(texture);
            }
            if !self.cache.is_null() {
                CFRelease(self.cache);
            }
            if !self.transfer.is_null() {
                VTPixelTransferSessionInvalidate(self.transfer);
                CFRelease(self.transfer);
            }
        }
    }
}

impl Feed {
    /// Called by SceneKit on its render thread before each frame.
    fn update(&self, device: *mut AnyObject) {
        let lit = self.lit.load(Ordering::Relaxed);
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let state = &mut *state;
        if lit != state.showing {
            let black = color(0., 0., 0., 1.);
            for material in [&state.inner.0, &state.cover.0] {
                set_contents(material, "emission", &black);
            }
            state.showing = lit;
            state.last = 0;
        }
        let material = match lit as u32 {
            INNER_ID => state.inner.0.clone(),
            COVER_ID => state.cover.0.clone(),
            _ => return,
        };
        let Some(frame) = self.renderer.displayed() else {
            return;
        };
        let address = CFRetained::as_ptr(&frame).as_ptr() as usize;
        if address == state.last {
            return;
        }
        state.last = address;
        if state.cache.is_null() {
            if device.is_null() {
                return;
            }
            let mut cache = std::ptr::null_mut();
            let status = unsafe {
                CVMetalTextureCacheCreate(
                    std::ptr::null(),
                    std::ptr::null(),
                    device,
                    std::ptr::null(),
                    &mut cache,
                )
            };
            if status != 0 || cache.is_null() {
                return;
            }
            state.cache = cache;
        }
        let Some(bgra) = state.bgra(frame) else {
            return;
        };
        let (width, height) = (CVPixelBufferGetWidth(&bgra), CVPixelBufferGetHeight(&bgra));
        let mut texture = std::ptr::null_mut();
        let status = unsafe {
            CVMetalTextureCacheCreateTextureFromImage(
                std::ptr::null(),
                state.cache,
                CFRetained::as_ptr(&bgra).as_ptr(),
                std::ptr::null(),
                BGRA8_SRGB,
                width,
                height,
                0,
                &mut texture,
            )
        };
        if status != 0 || texture.is_null() {
            return;
        }
        let metal = unsafe { CVMetalTextureGetTexture(texture) };
        if let Some(metal) = unsafe { metal.as_ref() } {
            set_contents(&material, "emission", metal);
        }
        state.textures.push(texture);
        while state.textures.len() > 3 {
            unsafe { CFRelease(state.textures.remove(0)) };
        }
        unsafe { CVMetalTextureCacheFlush(state.cache, 0) };
    }
}

impl FeedState {
    /// `frame` as BGRA, which a Metal texture can be made of directly.
    fn bgra(&mut self, frame: CFRetained<CVPixelBuffer>) -> Option<CFRetained<CVPixelBuffer>> {
        if CVPixelBufferGetPixelFormatType(&frame) == kCVPixelFormatType_32BGRA {
            return Some(frame);
        }
        let (width, height) = (
            CVPixelBufferGetWidth(&frame),
            CVPixelBufferGetHeight(&frame),
        );
        if self.buffers.first().is_some_and(|b| {
            CVPixelBufferGetWidth(b) != width || CVPixelBufferGetHeight(b) != height
        }) {
            self.buffers.clear();
        }
        if self.buffers.len() < 3 {
            self.buffers
                .push(super::screen::bgra_buffer(width, height)?);
            self.next = self.buffers.len() - 1;
        } else {
            self.next = (self.next + 1) % self.buffers.len();
        }
        let target = self.buffers[self.next].clone();
        if self.transfer.is_null() {
            let mut session = std::ptr::null_mut();
            if unsafe { VTPixelTransferSessionCreate(std::ptr::null(), &mut session) } != 0
                || session.is_null()
            {
                return None;
            }
            self.transfer = session;
        }
        let status = unsafe {
            VTPixelTransferSessionTransferImage(
                self.transfer,
                CFRetained::as_ptr(&frame).as_ptr(),
                CFRetained::as_ptr(&target).as_ptr(),
            )
        };
        (status == 0).then_some(target)
    }
}

pub(crate) struct DelegateIvars {
    feed: Arc<Feed>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MonocodeDeviceModelDelegate"]
    #[ivars = DelegateIvars]
    struct ModelDelegate;

    impl ModelDelegate {
        #[unsafe(method(renderer:updateAtTime:))]
        fn update_at_time(&self, renderer: &AnyObject, _time: f64) {
            let device: *mut AnyObject = unsafe { msg_send![renderer, device] };
            self.ivars().feed.update(device);
        }
    }
);

impl ModelDelegate {
    fn new(feed: Arc<Feed>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { feed });
        unsafe { msg_send![super(this), init] }
    }
}

// ---------------------------------------------------------------------------
// The view.

/// What the model shows, from the panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Target {
    pub(crate) angle: f64,
    /// The pose the hub reports, or none for a custom angle.
    pub(crate) pose: Option<Pose>,
    /// The screen streaming, by serve-sim's id.
    pub(crate) screen: Option<u32>,
    /// Quick moves while the slider is dragged.
    pub(crate) dragging: bool,
}

pub(crate) struct Model {
    view: Retained<NSView>,
    scene: Scene,
    feed: Arc<Feed>,
    _delegate: Retained<ModelDelegate>,
    shown: Cell<Option<Layout>>,
    painted: Painted,
    placed: Cell<[f64; 5]>,
    /// The picture painted in the view's place while something covers it.
    still: RefCell<Option<CFRetained<CVPixelBuffer>>>,
}

impl Overlay for Model {
    fn step_aside(&self) {
        if !self.view.isHidden() {
            self.view.setHidden(true);
            let _: () = unsafe { msg_send![&*self.view, setPlaying: false] };
        }
    }
}

impl Model {
    pub(crate) fn new(screen: &Screen) -> Option<Rc<Self>> {
        MainThreadMarker::new()?;
        let scene = build_scene();
        let feed = Arc::new(Feed {
            renderer: screen.renderer(),
            lit: AtomicU8::new(0),
            state: Mutex::new(FeedState {
                inner: Shared(scene.inner.clone()),
                cover: Shared(scene.cover.clone()),
                cache: std::ptr::null_mut(),
                transfer: std::ptr::null_mut(),
                last: 0,
                buffers: Vec::new(),
                next: 0,
                textures: Vec::new(),
                showing: 0,
            }),
        });
        let delegate = ModelDelegate::new(feed.clone());
        let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(1., 1.));
        let none: Option<&AnyObject> = None;
        let alloc: objc2::rc::Allocated<NSView> = unsafe { msg_send![class!(SCNView), alloc] };
        let view: Retained<NSView> =
            unsafe { msg_send![alloc, initWithFrame: frame, options: none] };
        unsafe {
            let _: () = msg_send![&*view, setScene: &*scene.scene];
            let _: () = msg_send![&*view, setPointOfView: &*scene.camera];
            let _: () = msg_send![&*view, setAllowsCameraControl: true];
            let _: () = msg_send![&*view, setAntialiasingMode: 2usize];
            let _: () = msg_send![&*view, setPreferredFramesPerSecond: 60isize];
            let _: () = msg_send![&*view, setRendersContinuously: true];
            let _: () = msg_send![&*view, setDelegate: &*delegate];
            let controller: Id = msg_send![&*view, defaultCameraController];
            // Turntable: drag turns the device round and tips it, never
            // under the floor.
            let _: () = msg_send![&*controller, setInteractionMode: 1isize];
            let _: () = msg_send![&*controller, setTarget: v3(0., 42., 0.)];
            let _: () = msg_send![&*controller, setMinimumVerticalAngle: 4f32];
            let _: () = msg_send![&*controller, setMaximumVerticalAngle: 88f32];
        }
        view.setWantsLayer(true);
        view.setHidden(true);
        let model = Rc::new(Self {
            view,
            scene,
            feed,
            _delegate: delegate,
            shown: Cell::new(None),
            painted: Default::default(),
            placed: Cell::new([f64::NAN; 5]),
            still: RefCell::new(None),
        });
        let overlay: Rc<dyn Overlay> = model.clone();
        track_overlay(&overlay, &model.painted);
        Some(model)
    }

    fn show(&self, target: Target) {
        let pose = target.pose.unwrap_or_else(|| nearest(target.angle));
        let to = layout(target.angle, pose);
        let screen = target.screen.unwrap_or(if target.angle <= 54. {
            COVER_ID
        } else {
            INNER_ID
        });
        self.feed.lit.store(screen as u8, Ordering::Relaxed);
        if self.shown.get() == Some(to) {
            return;
        }
        let seconds = match self.shown.get() {
            None => 0.,
            Some(_) if target.dragging => 0.12,
            Some(_) => 0.6,
        };
        apply(&self.scene, &to, seconds);
        self.shown.set(Some(to));
    }

    /// Fills its parent with the model on `backdrop`, moving it to `target`.
    /// The device's video view is placed beneath, at the picture's size, so
    /// it keeps decoding. While a popover covers the model it steps aside and
    /// a snapshot is painted in its place.
    pub(crate) fn element(
        self: &Rc<Self>,
        screen: Rc<Screen>,
        aspect: f32,
        rotation: monocode_core::device::protocol::Rotation,
        corner: f32,
        backdrop: gpui::Hsla,
        target: Target,
    ) -> gpui::AnyElement {
        let model = self.clone();
        canvas(
            move |bounds, window, _| {
                (
                    bounds,
                    window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
                )
            },
            move |_, (bounds, hitbox), window, _| {
                mark_painted(&model.painted, window);
                window.paint_quad(gpui::fill(bounds, backdrop).corner_radii(gpui::px(corner)));
                model.show(target);
                if window.is_hitbox_covered(&hitbox) {
                    if model.still.borrow().is_none() {
                        *model.still.borrow_mut() = model.snapshot();
                    }
                    model.step_aside();
                    screen.step_aside();
                    if let Some(still) = model.still.borrow().clone() {
                        window.paint_surface(bounds, still);
                    }
                    return;
                }
                model.still.take();
                let fitted = super::screen::fit(bounds, aspect);
                if !screen.place_beneath(fitted, rotation, window) {
                    model.step_aside();
                    return;
                }
                model.place(bounds, corner, backdrop, &screen, window);
            },
        )
        .size_full()
        .into_any_element()
    }

    fn place(
        &self,
        bounds: gpui::Bounds<gpui::Pixels>,
        corner: f32,
        backdrop: gpui::Hsla,
        screen: &Screen,
        window: &mut Window,
    ) {
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        let parent = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
        let zoom = window.content_zoom();
        let bounds = bounds.map(|v| v * zoom);
        let (x, y, width, height) = (
            f64::from(bounds.origin.x),
            f64::from(bounds.origin.y),
            f64::from(bounds.size.width),
            f64::from(bounds.size.height),
        );
        let y = if parent.isFlipped() {
            y
        } else {
            parent.bounds().size.height - y - height
        };
        let attached =
            unsafe { self.view.superview() }.is_some_and(|sup| std::ptr::eq(&*sup, parent));
        if !attached {
            parent.addSubview_positioned_relativeTo(
                &self.view,
                NSWindowOrderingMode::Above,
                Some(screen.view()),
            );
        }
        let placed = [x, y, width, height, f64::from(corner * zoom)];
        if self.placed.get() != placed {
            self.view
                .setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)));
            let layer: Option<Retained<objc2_quartz_core::CALayer>> =
                unsafe { msg_send![&*self.view, layer] };
            if let Some(layer) = layer {
                layer.setCornerRadius(placed[4]);
                layer.setMasksToBounds(true);
            }
            self.placed.set(placed);
        }
        let rgba = backdrop.to_rgb();
        let _: () = unsafe {
            msg_send![&*self.view, setBackgroundColor: &*color(rgba.r as f64, rgba.g as f64, rgba.b as f64, 1.)]
        };
        if self.view.isHidden() {
            self.view.setHidden(false);
            let _: () = unsafe { msg_send![&*self.view, setPlaying: true] };
        }
    }

    /// What the view last drew, as a frame GPUI can paint.
    fn snapshot(&self) -> Option<CFRetained<CVPixelBuffer>> {
        if self.view.isHidden() {
            return None;
        }
        let image: Option<Id> = unsafe { msg_send![&*self.view, snapshot] };
        let image = image?;
        image_frame(&image)
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: *const c_void;
    fn CGColorSpaceCreateWithName(name: *const c_void) -> *mut c_void;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: *mut c_void,
        info: u32,
    ) -> *mut c_void;
    fn CGContextDrawImage(context: *mut c_void, rect: NSRect, image: *const c_void);
    fn CGImageGetWidth(image: *const c_void) -> usize;
    fn CGImageGetHeight(image: *const c_void) -> usize;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVPixelBufferLockBaseAddress(buffer: *const CVPixelBuffer, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: *const CVPixelBuffer, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buffer: *const CVPixelBuffer) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(buffer: *const CVPixelBuffer) -> usize;
}

/// `image` drawn into a BGRA frame GPUI can paint.
fn image_frame(image: &AnyObject) -> Option<CFRetained<CVPixelBuffer>> {
    let none: Option<&AnyObject> = None;
    let cg: *const c_void = unsafe {
        msg_send![image, CGImageForProposedRect: std::ptr::null_mut::<NSRect>(), context: none, hints: none]
    };
    if cg.is_null() {
        return None;
    }
    let (width, height) = unsafe { (CGImageGetWidth(cg), CGImageGetHeight(cg)) };
    let frame = super::screen::bgra_buffer(width.max(1), height.max(1))?;
    let raw = CFRetained::as_ptr(&frame).as_ptr().cast_const();
    unsafe {
        if CVPixelBufferLockBaseAddress(raw, 0) != 0 {
            return None;
        }
        let space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        // Premultiplied BGRA: alpha first, 32-bit little endian.
        let context = CGBitmapContextCreate(
            CVPixelBufferGetBaseAddress(raw),
            width,
            height,
            8,
            CVPixelBufferGetBytesPerRow(raw),
            space,
            2 | (2 << 12),
        );
        if !context.is_null() {
            CGContextDrawImage(
                context,
                NSRect::new(
                    NSPoint::new(0., 0.),
                    NSSize::new(width as f64, height as f64),
                ),
                cg,
            );
            CFRelease(context);
        }
        if !space.is_null() {
            CFRelease(space);
        }
        CVPixelBufferUnlockBaseAddress(raw, 0);
    }
    Some(frame)
}

impl Drop for Model {
    fn drop(&mut self) {
        let none: Option<&AnyObject> = None;
        let _: () = unsafe { msg_send![&*self.view, setDelegate: none] };
        self.view.removeFromSuperview();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlines_close_round_their_corners() {
        let (start, segments) = outline([0., 0.], [10., 20.], [2., 2., 2., 2.]);
        let points = flatten(start, &segments, 4);
        assert_eq!(points[0], [2., 20.]);
        for p in &points {
            assert!(p[0] >= -1e-9 && p[0] <= 10. + 1e-9 && p[1] >= -1e-9 && p[1] <= 20. + 1e-9);
        }
        // A square corner adds no curve.
        let (_, square) = outline([0., 0.], [10., 20.], [0., 2., 2., 0.]);
        assert_eq!(square.len(), 4 + 3 + 3);
    }

    #[test]
    fn the_device_rests_on_the_floor_in_every_pose() {
        for pose in Pose::ALL {
            let to = layout(pose.angle(), pose);
            assert!(to.offset[1] > -200. && to.offset[1] < 200., "{pose:?}");
            let n: f64 = to.holding.iter().map(|v| v * v).sum();
            assert!((n - 1.).abs() < 1e-9, "{pose:?}");
        }
        // Folded, it is two halves thick; open flat, one.
        let closed = layout(0., Pose::Closed);
        let open = layout(180., Pose::Open);
        assert!((closed.offset[1] - HALF_T - 0.2).abs() < 0.01, "{closed:?}");
        assert!((open.offset[1] - HALF_T - 0.2).abs() < 1e-6, "{open:?}");
    }

    /// A test card in a framebuffer's shape: its quarters red, green, blue
    /// and yellow from the top left, so a render shows which way it lies.
    fn card(width: usize, height: usize) -> Id {
        let mut pixels = vec![0u8; width * height * 4];
        for row in 0..height {
            // BMP rows run bottom up.
            let top = row >= height / 2;
            for col in 0..width {
                let left = col < width / 2;
                let [r, g, b] = match (top, left) {
                    (true, true) => [230, 40, 40],
                    (true, false) => [40, 200, 60],
                    (false, true) => [40, 80, 230],
                    (false, false) => [240, 210, 40],
                };
                let i = (row * width + col) * 4;
                pixels[i..i + 4].copy_from_slice(&[b, g, r, 255]);
            }
        }
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&((54 + pixels.len()) as u32).to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&(width as i32).to_le_bytes());
        bmp.extend_from_slice(&(height as i32).to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&32u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        bmp.extend_from_slice(&[0; 16]);
        bmp.extend_from_slice(&pixels);
        let data = objc2_foundation::NSData::with_bytes(&bmp);
        let alloc: objc2::rc::Allocated<AnyObject> = unsafe { msg_send![class!(NSImage), alloc] };
        unsafe { msg_send![alloc, initWithData: &*data] }
    }

    #[link(name = "Metal", kind = "framework")]
    unsafe extern "C" {
        fn MTLCreateSystemDefaultDevice() -> *mut AnyObject;
    }

    /// Renders every pose to PNGs in `MONOCODE_MODEL_RENDERS`, to look at.
    #[test]
    #[ignore = "writes renders to look at"]
    fn renders_each_pose() {
        let dir = std::env::var("MONOCODE_MODEL_RENDERS").expect("MONOCODE_MODEL_RENDERS");
        let scene = build_scene();
        set_contents(&scene.inner, "emission", &card(200, 284));
        set_contents(&scene.cover, "emission", &card(140, 203));
        let device = unsafe { MTLCreateSystemDefaultDevice() };
        let none: Option<&AnyObject> = None;
        let renderer: Id =
            unsafe { msg_send![class!(SCNRenderer), rendererWithDevice: device, options: none] };
        unsafe {
            let _: () = msg_send![&*renderer, setScene: &*scene.scene];
            let _: () = msg_send![&*renderer, setPointOfView: &*scene.camera];
        }
        for (name, angle, pose) in [
            ("open", 180., Pose::Open),
            ("book", 90., Pose::Book),
            ("laptop", 90., Pose::Laptop),
            ("tent", 80., Pose::Tent),
            ("closed", 0., Pose::Closed),
            ("custom120", 120., Pose::Book),
        ] {
            apply(&scene, &layout(angle, pose), 0.);
            let image: Id = unsafe {
                msg_send![&*renderer, snapshotAtTime: 0f64, withSize: NSSize::new(800., 600.), antialiasingMode: 2usize]
            };
            let tiff: Id = unsafe { msg_send![&*image, TIFFRepresentation] };
            let rep: Id = unsafe { msg_send![class!(NSBitmapImageRep), imageRepWithData: &*tiff] };
            let empty = objc2_foundation::NSDictionary::<NSString, AnyObject>::new();
            let png: Id =
                unsafe { msg_send![&*rep, representationUsingType: 4usize, properties: &*empty] };
            let path = NSString::from_str(&format!("{dir}/{name}.png"));
            let ok: bool = unsafe { msg_send![&*png, writeToFile: &*path, atomically: true] };
            assert!(ok, "{name}");
        }
    }

    #[test]
    fn poses_hold_the_device_as_named() {
        // Folded, the cover (the left half's back, -x) faces up.
        let up = rotate(holding(Pose::Closed), [-1., 0., 0.]);
        assert!((up[1] - 1.).abs() < 1e-9, "{up:?}");
        // Open, the inner display faces up.
        let up = rotate(holding(Pose::Open), [0., 0., 1.]);
        assert!((up[1] - 1.).abs() < 1e-9, "{up:?}");
        // In a tent the hinge's halves point down.
        let down = rotate(holding(Pose::Tent), [0., 0., 1.]);
        assert!((down[1] + 1.).abs() < 1e-9, "{down:?}");
    }
}
