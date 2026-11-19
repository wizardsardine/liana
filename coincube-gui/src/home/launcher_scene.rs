//! The Launcher floor as the prototype's CSS 3D scene, evaluated natively.
//!
//! `design-review/canvas/P-Launcher.dc.html` places every Cube in its own
//! `.scene` (`perspective: 1100px`) and animates one transform list,
//! `translate3d(dx, dy, 0) scale(k) rotateX(a) rotateY(b)`, together with
//! the scene's `perspective-origin`. This module reproduces that math exactly:
//! the same face transforms, the same per-function interpolation and easing,
//! the same perspective divide about a moving origin, and the same 2D scene
//! dimming. [`super::launcher_gpu`] rasterizes the result on the GPU with
//! perspective-correct face art, inset borders and blurred shadows.

use std::{
    fmt::Write,
    time::{Duration, Instant},
};

use super::{launcher_coin, CubeCatalogKey};
use crate::chain::{ChainId, ChainIdExt};

/// `.cube { transition: transform 820ms var(--spin) }`.
pub(super) const TURN: Duration = Duration::from_millis(820);
/// `.cb.handoff .cube { transition-delay: 160ms }`.
pub(super) const HANDOFF: Duration = Duration::from_millis(160);
/// `.scene { transition: opacity 500ms, transform 500ms }` and `.shadow`.
pub(super) const PEERS: Duration = Duration::from_millis(500);
/// `T.s`: one Cube size.
pub(super) const REST_SIZE: f32 = 152.0;
/// `.scene { perspective: 1100px }`.
pub(super) const PERSPECTIVE: f32 = 1100.0;
/// The details and unlock faces project to this many pixels on screen.
pub(super) const FACE_SIZE: f32 = 420.0;
/// `.fc.load`: the loading face, 1.67x the credentials face.
pub(super) const OPEN_FACE_SIZE: f32 = 700.0;
/// The finale's `scale(x3.6)` of the loading face.
const RUSH_SCALE: f32 = 3.6;
/// `.f { border: 1.5px }`, in face-local CSS pixels.
const BORDER: f32 = 1.5;

/// The prototype's 1440 x 900 board: stage below the 64px bar, 742px tall.
#[cfg(test)]
pub(super) const DESIGN_STAGE_WIDTH: f32 = 1440.0;
pub(super) const DESIGN_STAGE_HEIGHT: f32 = 742.0;
/// `T.slot`, `.rows.big .row { gap }`, `.rows.big { padding-top }`.
pub(super) const SLOT: f32 = 232.0;
pub(super) const SLOT_GAP: f32 = 40.0;
pub(super) const SLOT_HEIGHT: f32 = 344.0;
/// Kept clear on each side of the row for the page arrows: an 18px inset,
/// the 44px button and 18px of air.
pub(super) const ROW_GUTTER: f32 = 80.0;
/// `.floor` with `clip-path: polygon(14% 0, 86% 0, 100% 100%, 0 100%)`.
/// The prototype's `top: 40%` put the horizon at the resting Cubes' center
/// (300 of 742px); [`Layout::horizon`] keeps it there wherever the row sits.
pub(super) const FLOOR_INSET: f32 = 0.14;
/// `.cb { padding-bottom: 10px }`: the label's foot above the slot's.
const LABEL_FOOT: f32 = 10.0;
/// A hovered resting Cube floats this many pixels up and back, once a
/// [`BOB_PERIOD`], and settles over [`BOB_SETTLE`] when the pointer leaves.
pub(super) const BOB_HEIGHT: f32 = 6.0;
pub(super) const BOB_PERIOD: Duration = Duration::from_millis(2000);
pub(super) const BOB_SETTLE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy)]
pub(super) struct FaceText<'a> {
    pub name: &'a str,
    pub lock: &'a str,
    pub backup: &'a str,
    pub connect: &'a str,
    pub wallets: &'a str,
    pub passkey: bool,
}

/// Persisted facts drawn on a resting Cube. Nothing here needs an unlock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CubeMarks {
    pub network: ChainId,
    pub vault: bool,
    pub imported_bitcoin: bool,
    /// Spark comes with every Cube on a network it supports.
    pub spark: bool,
    pub passkey: bool,
    pub backup: BackupMark,
    /// Set for a Cube registered in Connect with no copy on this device.
    pub ghost: Option<Ghost>,
}

impl CubeMarks {
    /// A ghost knows its network, whether it can be restored here, and the
    /// wallets Connect knows of: Spark from its network, as on a local Cube,
    /// and a Vault once a device reported one ([`CubeMarks::with_vault`]).
    pub(crate) fn ghost(network: ChainId, ghost: Ghost) -> Self {
        Self {
            network,
            vault: false,
            imported_bitcoin: false,
            spark: crate::app::features::spark(network).is_available(),
            passkey: false,
            backup: BackupMark::Confirmed,
            ghost: Some(ghost),
        }
    }

    pub(crate) fn with_vault(self, vault: bool) -> Self {
        Self { vault, ..self }
    }
}

/// P-Launcher's `.cube.ghost`: a Cube registered in Connect but not on this
/// device, drawn see-through in its network's edge colour after the local
/// Cubes. Its one face is the details face; it never turns to unlock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Ghost {
    /// `.ghost.kit`: Connect holds a Recovery Kit with the seed, so this
    /// device can restore the Cube. The faces carry a faint fill.
    Restorable,
    /// `.ghost.nokit`: nothing on Connect restores it here. The faces are
    /// empty. The prototype dots these edges, but the floor already dashes
    /// test-network edges, so the two kinds differ by fill and mark instead
    /// (Robert, 2026-10-29).
    Unrestorable,
}

/// The right face's backup badge, P-Launcher's `.bdg`: "!" while the seed
/// backup is not confirmed, "?" when this device cannot tell, and none once
/// it is confirmed, so a badge always asks for attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum BackupMark {
    Confirmed,
    NotConfirmed,
    Unknown,
}

/// Which `.cb` phase the timeline describes. Every Cube on a page reads the
/// same timeline, so the 500ms dimming and 160ms handoff stay synchronized.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    /// Browse -> details on `selected`, or a handoff from `previous`.
    Select { previous: Option<CubeCatalogKey> },
    /// Details -> browse: the Cube spins back and the peers brighten.
    Return { key: CubeCatalogKey },
}

#[derive(Debug, Clone)]
pub(super) struct Motion {
    kind: Kind,
    started: Instant,
}

impl Motion {
    pub fn new(previous: Option<CubeCatalogKey>) -> Self {
        Self::at(previous, Instant::now())
    }

    pub fn at(previous: Option<CubeCatalogKey>, started: Instant) -> Self {
        Self {
            kind: Kind::Select { previous },
            started,
        }
    }

    pub fn returning(key: CubeCatalogKey) -> Self {
        Self::returning_at(key, Instant::now())
    }

    pub fn returning_at(key: CubeCatalogKey, started: Instant) -> Self {
        Self {
            kind: Kind::Return { key },
            started,
        }
    }

    /// The outgoing Cube of a handoff, still on the floor while it turns back.
    pub fn previous(&self) -> Option<&CubeCatalogKey> {
        match &self.kind {
            Kind::Select { previous } => previous.as_ref(),
            Kind::Return { .. } => None,
        }
    }

    /// The Cube returning to rest after its details closed.
    pub fn returning_key(&self) -> Option<&CubeCatalogKey> {
        match &self.kind {
            Kind::Return { key } => Some(key),
            Kind::Select { .. } => None,
        }
    }

    pub fn done(&self, now: Instant) -> bool {
        let delay = match &self.kind {
            Kind::Select {
                previous: Some(_), ..
            } => HANDOFF,
            _ => Duration::ZERO,
        };
        now.saturating_duration_since(self.started) >= (TURN + delay).max(PEERS)
    }

    fn elapsed(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.started)
    }

    /// The pose of one Cube on the page at `now`.
    pub fn pose(
        &self,
        key: &CubeCatalogKey,
        selected: Option<&CubeCatalogKey>,
        now: Instant,
    ) -> Pose {
        let t = self.elapsed(now);
        let turn =
            |delay: Duration| ease(t.saturating_sub(delay).as_secs_f32() / TURN.as_secs_f32());
        let fade = ease(t.as_secs_f32() / PEERS.as_secs_f32());
        match &self.kind {
            Kind::Select { previous } => {
                selection_pose(key, selected, previous.as_ref(), turn, fade)
            }
            Kind::Return { key: returning } => {
                if returning == key {
                    Pose {
                        spin: 1.0 - turn(Duration::ZERO),
                        second: 0.0,
                        dim: 0.0,
                        shadow: fade,
                        lift: 0.0,
                        open: 0.0,
                        rush: 0.0,
                    }
                } else {
                    Pose {
                        dim: 1.0 - fade,
                        ..Pose::REST
                    }
                }
            }
        }
    }
}

fn selection_pose(
    key: &CubeCatalogKey,
    selected: Option<&CubeCatalogKey>,
    previous: Option<&CubeCatalogKey>,
    turn: impl Fn(Duration) -> f32,
    fade: f32,
) -> Pose {
    if selected == Some(key) {
        let handoff = previous.is_some();
        Pose {
            spin: turn(if handoff { HANDOFF } else { Duration::ZERO }),
            second: 0.0,
            // A handoff target was dimmed; a browse selection was not.
            dim: if handoff { 1.0 - fade } else { 0.0 },
            shadow: 1.0 - fade,
            lift: 0.0,
            open: 0.0,
            rush: 0.0,
        }
    } else if previous == Some(key) {
        Pose {
            spin: 1.0 - turn(Duration::ZERO),
            second: 0.0,
            dim: fade,
            shadow: fade,
            lift: 0.0,
            open: 0.0,
            rush: 0.0,
        }
    } else {
        Pose {
            dim: if previous.is_some() { 1.0 } else { fade },
            ..Pose::REST
        }
    }
}

/// The settled pose of a Cube when no motion is running.
pub(super) fn settled_pose(key: &CubeCatalogKey, selected: Option<&CubeCatalogKey>) -> Pose {
    match selected {
        Some(selected) if selected == key => Pose::DETAILS,
        Some(_) => Pose::DIMMED,
        None => Pose::REST,
    }
}

/// One Cube's animated inputs. `spin` drives the shared transform list from
/// the resting pose (0) to the centered details face (1). `second` turns that
/// face on to the unlock face (yaw 180 -> 270). `dim` and `shadow` follow the
/// 500ms `.cb.dim .scene` and `.cb.active .shadow` transitions. `lift`
/// raises a resting Cube off its shadow by that many design pixels (hover).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Pose {
    pub spin: f32,
    pub second: f32,
    pub dim: f32,
    pub shadow: f32,
    pub lift: f32,
    /// The tumble from the unlock face to the loading face (0..1, eased).
    /// On a peer it steps the scene further back, `.cb.dim.deep`.
    pub open: f32,
    /// The finale: the loading face rushing toward the viewer (0..1, eased).
    pub rush: f32,
}

impl Pose {
    pub const REST: Self = Self {
        spin: 0.0,
        second: 0.0,
        dim: 0.0,
        shadow: 1.0,
        lift: 0.0,
        open: 0.0,
        rush: 0.0,
    };
    pub const DETAILS: Self = Self {
        spin: 1.0,
        second: 0.0,
        dim: 0.0,
        shadow: 0.0,
        lift: 0.0,
        open: 0.0,
        rush: 0.0,
    };
    pub const DIMMED: Self = Self {
        spin: 0.0,
        second: 0.0,
        dim: 1.0,
        shadow: 1.0,
        lift: 0.0,
        open: 0.0,
        rush: 0.0,
    };

    pub fn unlock(second: f32) -> Self {
        Self {
            second: second.clamp(0.0, 1.0),
            ..Self::DETAILS
        }
    }

    pub fn lifted(self, lift: f32) -> Self {
        Self { lift, ..self }
    }

    /// The unlocked Cube on its way to, or on, the loading face: `open` is
    /// the tumble, `rush` the finale.
    pub fn opening(open: f32, rush: f32) -> Self {
        Self {
            open: open.clamp(0.0, 1.0),
            rush: rush.clamp(0.0, 1.0),
            ..Self::unlock(1.0)
        }
    }

    /// A peer behind an opening Cube: `.cb.dim.deep`.
    pub fn deep(open: f32) -> Self {
        Self {
            open: open.clamp(0.0, 1.0),
            ..Self::DIMMED
        }
    }
}

/// The hover float `hovered` into it, eased up from rest so it never jumps:
/// half a cosine up to [`BOB_HEIGHT`] and back down, once a [`BOB_PERIOD`].
pub(super) fn bob(hovered: Duration) -> f32 {
    let phase = hovered.as_secs_f32() / BOB_PERIOD.as_secs_f32() * std::f32::consts::TAU;
    BOB_HEIGHT * (1.0 - phase.cos()) / 2.0
}

/// Settling from `from` after the pointer left `left` ago.
pub(super) fn settle(from: f32, left: Duration) -> f32 {
    from * (1.0 - ease(left.as_secs_f32() / BOB_SETTLE.as_secs_f32()))
}

/// `cubic-bezier(0.22, 1, 0.36, 1)`, inverted from x to y like CSS.
pub(super) fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t == 0.0 || t == 1.0 {
        return t;
    }
    let mut lo: f32 = 0.0;
    let mut hi: f32 = 1.0;
    for _ in 0..24 {
        let u = (lo + hi) * 0.5;
        let x = 3.0 * (1.0 - u).powi(2) * u * 0.22 + 3.0 * (1.0 - u) * u * u * 0.36 + u.powi(3);
        if x < t {
            lo = u;
        } else {
            hi = u;
        }
    }
    let u = (lo + hi) * 0.5;
    3.0 * (1.0 - u).powi(2) * u + 3.0 * (1.0 - u) * u * u + u.powi(3)
}

pub(super) fn second_turn_progress(started: Instant, now: Instant) -> f32 {
    ease(now.saturating_duration_since(started).as_secs_f32() / TURN.as_secs_f32())
}

/// `cubic-bezier(x1, y1, x2, y2)` at `t`, inverted from x to y like CSS.
pub(super) fn bezier(t: f32, (x1, y1, x2, y2): (f32, f32, f32, f32)) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t == 0.0 || t == 1.0 {
        return t;
    }
    let curve = |u: f32, a: f32, b: f32| {
        3.0 * (1.0 - u).powi(2) * u * a + 3.0 * (1.0 - u) * u * u * b + u.powi(3)
    };
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    for _ in 0..24 {
        let u = (lo + hi) * 0.5;
        if curve(u, x1, x2) < t {
            lo = u;
        } else {
            hi = u;
        }
    }
    curve((lo + hi) * 0.5, y1, y2)
}

/// `.cb.finale .cube { transition: transform 950ms cubic-bezier(.55,0,.9,.4) }`:
/// ease-in, so the face accelerates toward the viewer.
pub(super) const RUSH_EASE: (f32, f32, f32, f32) = (0.55, 0.0, 0.9, 0.4);

/// Stage geometry for a width. The prototype only defines the 1440px board;
/// narrower windows scale the row and Cube uniformly so a page of Cubes still
/// fits, while the focused face stays 420px whenever the stage is tall enough.
/// The row and the focused face are centered on the stage's midline, the line
/// the page arrows are centered on, at every stage size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Layout {
    pub width: f32,
    pub height: f32,
    pub scale: f32,
}

impl Layout {
    pub fn new(width: f32, height: f32) -> Self {
        let columns = super::launcher_catalog::floor_page_size(width) as f32;
        let row = columns * SLOT + (columns - 1.0) * SLOT_GAP;
        let scale = ((width - 2.0 * ROW_GUTTER) / row)
            .min(height / DESIGN_STAGE_HEIGHT)
            .clamp(0.6, 1.0);
        Self {
            width,
            height,
            scale,
        }
    }

    pub fn size(&self) -> f32 {
        REST_SIZE * self.scale
    }

    pub fn slot(&self) -> f32 {
        SLOT * self.scale
    }

    pub fn gap(&self) -> f32 {
        SLOT_GAP * self.scale
    }

    /// Top of the slots. A resting Cube and its name label read as one
    /// block, from the Cube's top corner to the label's foot, and that block
    /// is centered on the stage's midline. Centering the Cube alone left the
    /// label hanging below it and the row looking low.
    pub fn rows_top(&self) -> f32 {
        let (_, top, _, _) = bounds(self, (0.0, 0.0), Pose::REST);
        let foot = self.slot_height() - LABEL_FOOT - self.rest_drop();
        self.height / 2.0 - (top + foot) / 2.0 - self.rest_drop()
    }

    /// The resting Cubes' center line: the page arrows and the floor's back
    /// edge both sit on it.
    pub fn horizon(&self) -> f32 {
        self.rows_top() + self.rest_drop()
    }

    /// From the top of a slot down to its resting Cube's center.
    fn rest_drop(&self) -> f32 {
        8.0 * self.scale + 1.72 * self.size() * 0.475
    }

    /// `.cb` width x height for the labels and hit areas laid over the scene.
    pub fn slot_height(&self) -> f32 {
        SLOT_HEIGHT * self.scale
    }

    /// Rest center of the scene's Cube for slot `index` of `count`.
    pub fn rest_center(&self, index: usize, count: usize) -> (f32, f32) {
        let count = count.max(1) as f32;
        let row = count * self.slot() + (count - 1.0) * self.gap();
        let x0 = (self.width - row) / 2.0;
        (
            x0 + index as f32 * (self.slot() + self.gap()) + self.slot() / 2.0,
            self.horizon(),
        )
    }

    /// Where the details face is centered: on the arrows' line, as far as
    /// the face still clears the stage's edges by 24px.
    pub fn target(&self) -> (f32, f32) {
        let half = self.face() / 2.0 + 24.0;
        let y = self
            .horizon()
            .min(self.height - half)
            .max(half.min(self.height / 2.0));
        (self.width / 2.0, y)
    }

    /// The face size on screen at the target, bounded by the stage height.
    pub fn face(&self) -> f32 {
        FACE_SIZE.min(self.height - 48.0).max(200.0)
    }

    /// The loading face: 700px, the most that fits between the bars, and
    /// never smaller than the unlock face it grows from.
    pub fn open_face(&self) -> f32 {
        OPEN_FACE_SIZE.min(self.height - 44.0).max(self.face())
    }

    /// The loading face is centred on the stage.
    pub fn open_target(&self) -> (f32, f32) {
        (self.width / 2.0, self.height / 2.0)
    }
}

#[derive(Debug, Clone, Copy)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

impl Vec3 {
    const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// CSS `rotateX(a)` in its y-down frame.
    fn rotate_x(self, a: f32) -> Self {
        let (s, c) = a.sin_cos();
        Self::new(self.x, self.y * c - self.z * s, self.y * s + self.z * c)
    }

    /// CSS `rotateY(a)` in its y-down frame.
    fn rotate_y(self, a: f32) -> Self {
        let (s, c) = a.sin_cos();
        Self::new(self.x * c + self.z * s, self.y, -self.x * s + self.z * c)
    }
}

/// The five authored faces. `points` are the face element's own corners
/// (top-left, top-right, bottom-right, bottom-left) after its face transform,
/// in units of the Cube edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Face {
    /// `.f.t`: `rotateX(90deg) translateZ(s/2)`, network coin.
    Top,
    /// `.f.l`: `rotateY(-90deg) translateZ(s/2)`, wallet marks.
    Left,
    /// `.f.r`: `translateZ(s/2)`, lock and backup badge.
    Right,
    /// `.f.bl`: `rotateY(180deg) translateZ(s/2)`, details.
    BackLeft,
    /// `.f.br`: `rotateY(90deg) translateZ(s/2)`, unlock.
    BackRight,
    /// `.f.bt`: `rotateX(-90deg) translateZ(s/2)`, loading. Only an opening
    /// Cube draws it: at rest it faces the floor.
    Bottom,
}

impl Face {
    pub const ALL: [Face; 5] = [
        Face::Top,
        Face::Left,
        Face::Right,
        Face::BackLeft,
        Face::BackRight,
    ];

    fn corner(self, local: Vec3) -> Vec3 {
        let pushed = Vec3::new(local.x, local.y, local.z + 0.5);
        match self {
            Face::Top => pushed.rotate_x(90.0_f32.to_radians()),
            Face::Left => pushed.rotate_y((-90.0_f32).to_radians()),
            Face::Right => pushed,
            Face::BackLeft => pushed.rotate_y(180.0_f32.to_radians()),
            Face::BackRight => pushed.rotate_y(90.0_f32.to_radians()),
            Face::Bottom => pushed.rotate_x((-90.0_f32).to_radians()),
        }
    }

    fn corners(self) -> [Vec3; 4] {
        [
            Vec3::new(-0.5, -0.5, 0.0),
            Vec3::new(0.5, -0.5, 0.0),
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(-0.5, 0.5, 0.0),
        ]
        .map(|p| self.corner(p))
    }
}

/// A projected face: screen corners (TL, TR, BR, BL) and the homogeneous
/// `w = (d - z) / d` of each, so the GPU interpolates face art in 3D.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ProjectedFace {
    pub face: Face,
    pub corners: [(f32, f32); 4],
    pub w: [f32; 4],
    /// The face edge on screen, for the border width and art resolution.
    pub size: f32,
}

impl ProjectedFace {
    pub fn area(&self) -> f32 {
        let p = self.corners;
        (0..4)
            .map(|i| p[i].0 * p[(i + 1) % 4].1 - p[(i + 1) % 4].0 * p[i].1)
            .sum::<f32>()
            * 0.5
    }
}

/// The 2D scene that surrounds the 3D Cube: its shadow and the dimming
/// transform, both in stage pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SceneFrame {
    pub opacity: f32,
    pub shadow_center: (f32, f32),
    pub shadow_radii: (f32, f32),
    pub shadow_sigma: f32,
    pub shadow_opacity: f32,
}

/// Evaluate `.scene` and `.cube` styles for one Cube.
pub(super) fn project(
    layout: &Layout,
    rest: (f32, f32),
    pose: Pose,
) -> (SceneFrame, Vec<ProjectedFace>) {
    project_faces(layout, rest, pose, false)
}

/// [`project`], keeping the faces that turn away when `see_through`: a
/// ghost's `backface-visibility: visible`, so its far edges show through.
/// Those come first, so the near faces paint over them.
pub(super) fn project_faces(
    layout: &Layout,
    rest: (f32, f32),
    pose: Pose,
    see_through: bool,
) -> (SceneFrame, Vec<ProjectedFace>) {
    let s = layout.size();
    let p = pose.spin.clamp(0.0, 1.0);
    // A resting Cube never reads the target, which is placed from the
    // resting Cubes' own bounds (`Layout::rows_top`).
    let (tx, ty) = if p > 0.0 { layout.target() } else { rest };
    // The tumble moves the face to the stage centre and grows it to the
    // loading face; the finale scales that on toward the viewer.
    let open = pose.open.clamp(0.0, 1.0);
    let (ox, oy) = layout.open_target();
    let (tx, ty) = (tx + (ox - tx) * open, ty + (oy - ty) * open);
    let face = layout.face() + (layout.open_face() - layout.face()) * open;
    let face = face * (1.0 + (RUSH_SCALE - 1.0) * pose.rush.clamp(0.0, 1.0));
    // translate3d(dx, dy, 0) scale(k). CSS `scale(k)` is two-dimensional:
    // it never stretches z, so the zoomed face still sits only s/2 in front
    // of the cube center. The prototype solved k as if z scaled, which makes
    // its face settle near 379px; the documented target, and the 1:1 live
    // controls laid over it, are 420px, so k is solved for that with the
    // same 2D-scale depth.
    let k_end = face * (PERSPECTIVE - s / 2.0) / PERSPECTIVE / s;
    let dx = (tx - rest.0) * p;
    let dy = (ty - rest.1) * p;
    let k = 1.0 + (k_end - 1.0) * p;
    // `rotateX(90deg) rotateY(360deg)`: the bottom face tips up to the
    // viewer while the yaw squares it, so the Cube tumbles diagonally.
    let pitch = (-35.0_f32 * (1.0 - p) + 90.0 * open * p).to_radians();
    let yaw = (45.0 + (180.0 + 90.0 * pose.second + 90.0 * open - 45.0) * p).to_radians();
    // perspective-origin moves from the scene center (cube center + .043s)
    // to the translated cube center with the same easing and delay.
    let origin = (rest.0 + dx, rest.1 + 0.043 * s * (1.0 - p) + dy);
    // `.cb.dim .scene { opacity: .4; transform: scale(.92) }` about the
    // scene's own center, after the perspective projection is flattened.
    let dim = pose.dim.clamp(0.0, 1.0);
    // `.cb.dim.deep .scene { opacity: .12 }` behind an opening Cube.
    let deep = if p > 0.0 { 0.0 } else { open * dim };
    let scene_scale = 1.0 - 0.08 * dim;
    let scene_center = (rest.0, rest.1 + 0.043 * s);
    let dimmed = |(x, y): (f32, f32)| {
        (
            scene_center.0 + (x - scene_center.0) * scene_scale,
            scene_center.1 + (y - scene_center.1) * scene_scale,
        )
    };
    // A hover lift is a plain translate: the Cube rises, its shadow stays.
    let lift = pose.lift * layout.scale * (1.0 - p);
    let origin = (origin.0, origin.1 - lift);
    let center = (rest.0 + dx, rest.1 + dy - lift);
    let mut faces = Vec::with_capacity(3);
    let mut back = Vec::new();
    let bottom = (p > 0.0 && open > 0.0).then_some(Face::Bottom);
    for which in Face::ALL.iter().copied().chain(bottom) {
        let mut corners = [(0.0, 0.0); 4];
        let mut w = [1.0; 4];
        for (i, point) in which.corners().iter().copied().enumerate() {
            let q = point.rotate_y(yaw).rotate_x(pitch);
            let (qx, qy, qz) = (center.0 + q.x * s * k, center.1 + q.y * s * k, q.z * s);
            let depth = (PERSPECTIVE - qz) / PERSPECTIVE;
            corners[i] = dimmed((
                origin.0 + (qx - origin.0) / depth,
                origin.1 + (qy - origin.1) / depth,
            ));
            w[i] = depth;
        }
        let projected = ProjectedFace {
            face: which,
            corners,
            w,
            size: s * k * scene_scale,
        };
        // `backface-visibility: hidden`.
        if projected.area() > 0.5 {
            faces.push(projected);
        } else if see_through {
            back.push(projected);
        }
    }
    back.append(&mut faces);
    let faces = back;
    let rise = (pose.lift / BOB_HEIGHT).clamp(0.0, 1.0) * (1.0 - p);
    let shadow = |v: f32| v * scene_scale * (1.0 - 0.08 * rise);
    let frame = SceneFrame {
        opacity: 1.0 - 0.6 * dim - 0.28 * deep,
        shadow_center: dimmed((rest.0 + 0.058 * s, rest.1 + 0.525 * s)),
        shadow_radii: (shadow(0.725 * s), shadow(0.31 * s)),
        shadow_sigma: shadow(0.055 * s),
        shadow_opacity: pose.shadow.clamp(0.0, 1.0) * (1.0 - 0.25 * rise),
    };
    (frame, faces)
}

/// The screen rectangle a posed Cube covers, for keeping labels that the
/// prototype's lead and outgoing Cubes would paint over out of view.
pub(super) fn bounds(layout: &Layout, rest: (f32, f32), pose: Pose) -> (f32, f32, f32, f32) {
    let (_, faces) = project(layout, rest, pose);
    faces.iter().flat_map(|f| f.corners).fold(
        (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
        |(x0, y0, x1, y1), (x, y)| (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
    )
}

/// The label strip at the foot of slot `index`.
pub(super) fn label_rect(layout: &Layout, index: usize, count: usize) -> (f32, f32, f32, f32) {
    let (cx, _) = layout.rest_center(index, count);
    let bottom = layout.rows_top() + layout.slot_height() - LABEL_FOOT;
    (
        cx - layout.slot() / 2.0,
        bottom - 40.0,
        cx + layout.slot() / 2.0,
        bottom,
    )
}

pub(super) fn overlaps(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

/// Face colors and edges from the prototype tokens, as sRGB bytes. The GPU
/// blends them in sRGB space so they match the browser's compositing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Palette {
    pub background: [u8; 3],
    pub floor: [u8; 3],
    pub floor_rule: [u8; 3],
    pub shadow: [u8; 4],
    /// `--line`, for a ghost's dashed footprint.
    pub line: [u8; 3],
    pub ink: [u8; 3],
    pub top: [u8; 3],
    pub left: [u8; 3],
    pub right: [u8; 3],
    pub back: [u8; 3],
}

impl Palette {
    pub fn new(light: bool) -> Self {
        if light {
            Self {
                background: [0xfa, 0xf9, 0xf6],
                floor: [0xef, 0xeb, 0xe3],
                floor_rule: [0xe0, 0xd9, 0xcd],
                shadow: [70, 52, 30, 77],
                line: [0xc4, 0xbe, 0xb4],
                ink: [0x20, 0x20, 0x20],
                top: [0xe6, 0xe0, 0xd5],
                left: [0xd6, 0xcf, 0xc2],
                right: [0xc9, 0xc1, 0xb2],
                back: [0xe6, 0xe0, 0xd5],
            }
        } else {
            Self {
                background: [0x16, 0x17, 0x16],
                // Lighter than the prototype's #1b1c1a, which sat within five
                // levels of the background: the floor's step matches light's.
                floor: [0x24, 0x25, 0x22],
                floor_rule: [0x3a, 0x3b, 0x37],
                shadow: [0, 0, 0, 153],
                line: [0x42, 0x42, 0x42],
                ink: [0xf5, 0xf0, 0xe8],
                top: [0x41, 0x42, 0x3e],
                left: [0x2e, 0x2f, 0x2c],
                right: [0x26, 0x27, 0x24],
                back: [0x2a, 0x2b, 0x28],
            }
        }
    }

    pub fn face(&self, face: Face) -> [u8; 3] {
        match face {
            Face::Top => self.top,
            Face::Left => self.left,
            Face::Right => self.right,
            // `--face-b` is the back faces' colour.
            Face::BackLeft | Face::BackRight | Face::Bottom => self.back,
        }
    }
}

/// `--glow`: the opening face's edge light, the network family hue at full
/// strength in both themes.
pub(crate) fn glow(network: ChainId) -> [u8; 3] {
    let [r, g, b, _] = edge(network, false);
    [r, g, b]
}

/// `--edge-btc`, `--edge-b2b`, `--edge-test`: the network family hue at 60%.
pub(super) fn edge(network: ChainId, light: bool) -> [u8; 4] {
    if network.is_blake2b() && !launcher_coin::is_test_network(network) {
        [212, 166, 42, 153]
    } else if matches!(network, ChainId::Bitcoin) {
        [243, 130, 29, 153]
    } else if light {
        [87, 90, 84, 153]
    } else {
        [185, 187, 181, 153]
    }
}

/// `--ghost-btc`, `--ghost-test`: a ghost's edges, the family hue at 50%
/// (55% for light Bitcoin, which darkens the hue to hold contrast).
pub(super) fn ghost_edge(network: ChainId, light: bool) -> [u8; 4] {
    if network.is_blake2b() && !launcher_coin::is_test_network(network) {
        // No ghost is listed on Blake2b; keep its hue if one ever is.
        if light {
            [140, 104, 10, 140]
        } else {
            [212, 166, 42, 128]
        }
    } else if matches!(network, ChainId::Bitcoin) {
        if light {
            [176, 86, 8, 140]
        } else {
            [243, 130, 29, 128]
        }
    } else if light {
        [87, 90, 84, 128]
    } else {
        [185, 187, 181, 128]
    }
}

/// `--ghost-fill`, the faint face of a restorable ghost, and the empty face
/// of one that cannot be restored, as sRGB plus alpha.
pub(super) fn ghost_fill(ghost: Ghost, light: bool) -> [u8; 4] {
    match (ghost, light) {
        (Ghost::Unrestorable, _) => [0, 0, 0, 0],
        (Ghost::Restorable, false) => [245, 240, 232, 10],
        (Ghost::Restorable, true) => [32, 32, 32, 9],
    }
}

/// The border in face-local pixels: 1.5px of an `s`-pixel face.
pub(super) fn border_fraction(edge_px: f32) -> f32 {
    BORDER / edge_px
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const SHIELD: &str = "M12 3l7 3v5.5c0 4.3-3 7.6-7 9.5-4-1.9-7-5.2-7-9.5V6z";
/// An imported wallet: a billfold, since a coin would read as the network
/// (Robert, 2026-11-07).
const IMPORTED: &str =
    "M4 7.5h15a1 1 0 0 1 1 1V19a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1zM4 7.5 15 4.2v3.3M16 14h1.5";
const LOCKED: &str = "M7 11h10v9H7zM9 11V8a3 3 0 0 1 6 0v3";
/// Spark's asterisk, the same path as `coincube-ui`'s `spark_badge.svg`. It
/// is a filled mark in a 67.3 x 64 box, so [`spark`] fits it to the 24-unit
/// icon grid instead of stroking it like the other marks.
const SPARK: &str = "M39.7159 25.248L40.8727 0.570312H26.4219L27.5787 25.2483L4.46555 16.5221L0 30.2656L23.8282 36.7915L8.38717 56.0763L20.0781 64.5703L33.6483 43.9245L47.2179 64.5695L58.9089 56.0755L43.4679 36.7909L67.2937 30.2657L62.8281 16.5221L39.7159 25.248Z";
const PASSKEY: &str = "M9 5a4 4 0 1 0 0 8a4 4 0 1 0 0-8M12 11.5l7 7m-3-3 2.5-2.5m-.5 5 2.5-2.5";
const CIRCLE: &str = "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18";
/// `.bdg.warn`: backup not confirmed.
const ATTENTION: &str = "M12 7.5v5.5M12 16.2v.3";
/// `.bdg.unk`: backup status unknown.
const QUESTION: &str = "M9.6 9.6a2.4 2.4 0 1 1 3.4 2.2c-.7.3-1 .8-1 1.5v.4M12 16.3v.3";

/// The Spark mark, filled, about 18 units wide and centered in the 24-unit
/// cell the stroked icons use.
fn spark(svg: &mut String, x: f32, y: f32, size: f32, ink: &str) {
    let k = size / 24.0;
    let _ = write!(
        svg,
        "<g transform=\"translate({x:.3} {y:.3}) scale({k:.5}) translate(3 3.4) scale(0.2675)\"><path d=\"{SPARK}\" fill=\"{ink}\" fill-rule=\"evenodd\"/></g>"
    );
}

fn icon(svg: &mut String, path: &str, x: f32, y: f32, size: f32, ink: &str) {
    let k = size / 24.0;
    let _ = write!(
        svg,
        "<g transform=\"translate({x:.3} {y:.3}) scale({k:.5})\"><path d=\"{path}\" fill=\"none\" stroke=\"{ink}\" stroke-width=\"1.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/></g>"
    );
}

/// Content of a resting face in its own CSS pixels (`0 0 s s`). Faces have
/// no background here: the GPU paints the face color and border under it.
pub(super) fn rest_face_svg(face: Face, marks: CubeMarks, light: bool) -> Option<String> {
    let s = REST_SIZE;
    let ink = hex(Palette::new(light).ink);
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {s} {s}\" width=\"{s}\" height=\"{s}\" color=\"{ink}\">"
    );
    if let Some(ghost) = marks.ghost {
        return ghost_face_svg(svg, face, marks, ghost, light);
    }
    match face {
        Face::Top => append_rest_top(&mut svg, marks.network, light, &ink),
        Face::Left => {
            // `.lc`: centered row of `.wi` marks (.25s, gap .04s), in the
            // prototype's Vault, Spark order. Spark is known without an
            // unlock: every Cube on a network that supports it has one.
            let mut paths = Vec::new();
            if marks.vault {
                paths.push(SHIELD);
            }
            if marks.imported_bitcoin {
                paths.push(IMPORTED);
            }
            if marks.spark {
                paths.push(SPARK);
            }
            if paths.is_empty() {
                return None;
            }
            append_wallet_icons(&mut svg, &paths, &ink);
        }
        Face::Right => append_rest_lock(&mut svg, marks, light, &ink),
        Face::BackLeft | Face::BackRight | Face::Bottom => return None,
    }
    svg.push_str("</svg>");
    Some(svg)
}

fn append_rest_top(svg: &mut String, network: ChainId, light: bool, ink: &str) {
    // `.tc { transform: rotate(45deg) }`: coin, then the test tag.
    let s = REST_SIZE;
    let coin = s * 0.34;
    let tag = launcher_coin::is_test_network(network).then(|| network.label());
    let font = s * 0.095;
    let tag_h = font * 1.2 + 2.0;
    let column = coin + tag.map_or(0.0, |_| s * 0.04 + tag_h);
    let top = (s - column) / 2.0;
    let _ = write!(
        svg,
        "<g transform=\"rotate(45 {:.2} {:.2})\">",
        s / 2.0,
        s / 2.0
    );
    svg.push_str(&launcher_coin::fragment(
        network,
        (s - coin) / 2.0,
        top,
        coin,
        1.5,
        s * 0.018,
    ));
    if let Some(tag) = tag {
        let width = tag.chars().count() as f32 * font * 0.56 + font * 0.7;
        let y = top + coin + s * 0.04;
        let edge = edge(network, light);
        let _ = write!(
            svg,
            "<rect x=\"{:.2}\" y=\"{y:.2}\" width=\"{width:.2}\" height=\"{tag_h:.2}\" rx=\"{:.2}\" fill=\"none\" stroke=\"rgb({},{},{})\" stroke-opacity=\"0.6\" stroke-dasharray=\"2 2\"/><text x=\"{:.2}\" y=\"{:.2}\" font-family=\"IBM Plex Sans\" font-weight=\"600\" font-size=\"{font:.2}\" text-anchor=\"middle\" fill=\"{ink}\">{}</text>",
            (s - width) / 2.0,
            font * 0.3,
            edge[0],
            edge[1],
            edge[2],
            s / 2.0,
            y + 1.0 + font * 0.95,
            xml_text(tag)
        );
    }
    svg.push_str("</g>");
}

fn append_wallet_icons(svg: &mut String, paths: &[&str], ink: &str) {
    let s = REST_SIZE;
    let size = s * 0.25;
    let gap = s * 0.04;
    let row = paths.len() as f32 * size + (paths.len() - 1) as f32 * gap;
    for (i, &path) in paths.iter().enumerate() {
        let x = (s - row) / 2.0 + i as f32 * (size + gap);
        let y = (s - size) / 2.0;
        if path == SPARK {
            spark(svg, x, y, size, ink);
        } else {
            icon(svg, path, x, y, size, ink);
        }
    }
}

fn append_rest_lock(svg: &mut String, marks: CubeMarks, light: bool, ink: &str) {
    // `.rc`: `.pad` (.36s) and the `.bdg` (.21s, margin-top -.03s).
    let s = REST_SIZE;
    let pad = s * 0.36;
    let badge = s * 0.21;
    let gap = s * 0.02;
    let warn = if light { "#77450b" } else { "#f3b465" };
    let muted = if light { "#575a54" } else { "#b9bbb5" };
    let mark = match marks.backup {
        BackupMark::Confirmed => None,
        BackupMark::NotConfirmed => Some((ATTENTION, warn)),
        BackupMark::Unknown => Some((QUESTION, muted)),
    };
    let row = if mark.is_some() {
        pad + gap + badge
    } else {
        pad
    };
    let x = (s - row) / 2.0;
    let y = (s - pad) / 2.0;
    icon(
        svg,
        if marks.passkey { PASSKEY } else { LOCKED },
        x,
        y,
        pad,
        ink,
    );
    if let Some((glyph, tone)) = mark {
        let path = format!("{CIRCLE}{glyph}");
        icon(svg, &path, x + pad + gap, y - s * 0.03, badge, tone);
    }
}

/// `KIT` and `NOKIT` from P-Launcher: a case with a check, and the same case
/// struck through.
pub(super) const KIT: &str =
    "M4 8.5h16V19H4zM9 8.5V6.5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2M8.8 13.6l2.2 2.2 4.2-4.3";
pub(super) const NOKIT: &str =
    "M4 8.5h16V19H4zM9 8.5V6.5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2M3 4l18 18";

/// A ghost's resting faces, P-Launcher's `.cube.ghost`: the coin greyed on
/// top (`filter: grayscale(1); opacity: .55`); on the left its wallet marks
/// and the cloud that says it lives in Connect, all muted; and the Recovery
/// Kit mark where a local Cube shows its lock, in `--good` when the kit
/// restores it, else muted beside the `--warn` badge.
fn ghost_face_svg(
    mut svg: String,
    face: Face,
    marks: CubeMarks,
    ghost: Ghost,
    light: bool,
) -> Option<String> {
    let s = REST_SIZE;
    let network = marks.network;
    let muted = if light { "#575a54" } else { "#b9bbb5" };
    match face {
        Face::Top => {
            let coin = s * 0.34;
            let _ = write!(
                svg,
                "<filter id=\"g\"><feColorMatrix type=\"saturate\" values=\"0\"/></filter><g transform=\"rotate(45 {:.2} {:.2})\" filter=\"url(#g)\" opacity=\".55\">",
                s / 2.0,
                s / 2.0
            );
            svg.push_str(&launcher_coin::fragment(
                network,
                (s - coin) / 2.0,
                (s - coin) / 2.0,
                coin,
                1.5,
                s * 0.018,
            ));
            svg.push_str("</g>");
        }
        Face::Left => {
            // `.lc`: the local Cube's row of `.wi` marks, then the cloud.
            let mut paths = Vec::new();
            if marks.vault {
                paths.push(SHIELD);
            }
            if marks.spark {
                paths.push(SPARK);
            }
            paths.push(CLOUD);
            append_wallet_icons(&mut svg, &paths, muted);
        }
        Face::Right => append_ghost_recovery_badge(&mut svg, ghost, light, muted),
        Face::BackLeft | Face::BackRight | Face::Bottom => return None,
    }
    svg.push_str("</svg>");
    Some(svg)
}

fn append_ghost_recovery_badge(svg: &mut String, ghost: Ghost, light: bool, muted: &str) {
    // As a local Cube's `.rc`: `.pad` (.36s), then for a ghost no
    // kit restores, the `.bdg` (.21s) in `--warn`.
    let s = REST_SIZE;
    let pad = s * 0.36;
    let badge = s * 0.21;
    let gap = s * 0.02;
    let (path, tone) = match ghost {
        Ghost::Restorable => (KIT, if light { "#23613b" } else { "#8bdfab" }),
        Ghost::Unrestorable => (NOKIT, muted),
    };
    let warn = ghost == Ghost::Unrestorable;
    let row = if warn { pad + gap + badge } else { pad };
    let x = (s - row) / 2.0;
    let y = (s - pad) / 2.0;
    icon(svg, path, x, y, pad, tone);
    if warn {
        let tone = if light { "#77450b" } else { "#f3b465" };
        let path = format!("{CIRCLE}{ATTENTION}");
        icon(svg, &path, x + pad + gap, y - s * 0.03, badge, tone);
    }
}

/// What a ghost's details face says, for the snapshot that rides the turn.
#[derive(Clone, Copy)]
pub(super) struct GhostText<'a> {
    pub name: &'a str,
    pub backup: &'a str,
    pub restorable: bool,
}

/// The ghost's details face (G-Ghost-Details `.fc`) as it turns into place,
/// before the live controls replace it: dashed cloud avatar, name, network,
/// the Where and Recovery facts and the primary action.
pub(super) fn ghost_details_svg(network: ChainId, light: bool, text: GhostText<'_>) -> String {
    let palette = Palette::new(light);
    let ink = hex(palette.ink);
    let muted = if light { "#575a54" } else { "#b9bbb5" };
    let line = if light { "#c4beb4" } else { "#424242" };
    let good = if light { "#23613b" } else { "#8bdfab" };
    let warn = if light { "#77450b" } else { "#f3b465" };
    let accent = if light { "#a14d00" } else { "#f3821d" };
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 420 420\" width=\"420\" height=\"420\" font-family=\"IBM Plex Sans\" color=\"{ink}\">"
    );
    let name: String = text.name.chars().take(26).collect();
    // `.av`: a 32px dashed ring holding the cloud.
    let _ = write!(
        svg,
        "<circle cx=\"44\" cy=\"44\" r=\"15.5\" fill=\"none\" stroke=\"{muted}\" stroke-dasharray=\"3 2\"/><text x=\"70\" y=\"51\" font-size=\"20\" font-weight=\"600\" fill=\"{ink}\">{}</text>",
        xml_text(&name)
    );
    icon(&mut svg, CLOUD, 36.0, 36.0, 16.0, muted);
    let _ = write!(
        svg,
        "<g opacity=\".6\">{}</g><text x=\"52\" y=\"83\" font-size=\"14\" fill=\"{ink}\">{}</text><path d=\"M28 108.5H392\" stroke=\"{line}\"/>",
        launcher_coin::fragment(network, 28.0, 69.0, 18.0, 1.0, 1.5),
        xml_text(super::launcher_chrome::network_name(network))
    );
    let tone = if text.restorable { good } else { warn };
    let backup: String = text.backup.chars().take(34).collect();
    let _ = write!(
        svg,
        "<text x=\"28\" y=\"141\" font-size=\"13\" fill=\"{muted}\">Where</text><text x=\"124\" y=\"141\" font-size=\"14\" fill=\"{muted}\">In Connect, not on this device</text><text x=\"28\" y=\"173\" font-size=\"13\" fill=\"{muted}\">Recovery</text><text x=\"124\" y=\"173\" font-size=\"14\" fill=\"{tone}\">{}</text>",
        xml_text(&backup)
    );
    append_ghost_primary_action(&mut svg, text.restorable, light, &ink, line, accent);
    svg
}

fn append_ghost_primary_action(
    svg: &mut String,
    restorable: bool,
    light: bool,
    ink: &str,
    line: &str,
    accent: &str,
) {
    let primary = if restorable {
        "Restore to this device"
    } else {
        "Import with recovery phrase…"
    };
    let width = primary.chars().count() as f32 * 7.6 + 32.0;
    let (fill, label) = if restorable {
        (accent, if light { "#ffffff" } else { "#201407" })
    } else {
        ("none", ink)
    };
    let _ = write!(
        svg,
        "<rect x=\"28.5\" y=\"334.5\" width=\"{width:.1}\" height=\"35\" rx=\"8\" fill=\"{fill}\" stroke=\"{}\"/><text x=\"{:.1}\" y=\"357\" text-anchor=\"middle\" font-size=\"14\" font-weight=\"500\" fill=\"{label}\">{}</text></svg>",
        if restorable { accent } else { line },
        28.5 + width / 2.0,
        xml_text(primary)
    );
}

/// `CLOUD` from P-Launcher: the ghost's avatar.
pub(super) const CLOUD: &str =
    "M7 18h10a4 4 0 0 0 .6-7.95A5.5 5.5 0 0 0 6.9 9.2 4.4 4.4 0 0 0 7 18z";

/// A snapshot of the live details panel laid out in the prototype's 420px
/// `.fc` box. It rides the turning face; the real Iced controls replace it
/// as soon as the face is square to the camera.
pub(super) fn details_svg(network: ChainId, light: bool, text: FaceText<'_>) -> String {
    let palette = Palette::new(light);
    let ink = hex(palette.ink);
    let muted = if light { "#575a54" } else { "#b9bbb5" };
    let line = if light { "#c4beb4" } else { "#424242" };
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 420 420\" width=\"420\" height=\"420\" font-family=\"IBM Plex Sans\" color=\"{ink}\">"
    );
    let initial: String = text
        .name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into());
    let name: String = text.name.chars().take(26).collect();
    let panel = if light { "#ffffff" } else { "#202020" };
    let accent = if light { "#a14d00" } else { "#f3821d" };
    // `.pn`: a 32px avatar, 10px gap, the name at 20px semibold.
    let _ = write!(
        svg,
        "<circle cx=\"44\" cy=\"44\" r=\"16\" fill=\"{panel}\"/><text x=\"44\" y=\"49\" text-anchor=\"middle\" font-size=\"14\" font-weight=\"600\" fill=\"{ink}\">{}</text><text x=\"70\" y=\"51\" font-size=\"20\" font-weight=\"600\" fill=\"{ink}\">{}</text>",
        xml_text(&initial),
        xml_text(&name)
    );
    // `.pnet`: the 18px coin and the network's full name, 8px below.
    let _ = write!(
        svg,
        "{}<text x=\"52\" y=\"83\" font-size=\"14\" fill=\"{ink}\">{}</text><path d=\"M28 108.5H392\" stroke=\"{line}\"/>",
        launcher_coin::fragment(network, 28.0, 69.0, 18.0, 1.0, 1.5),
        xml_text(super::launcher_chrome::network_name(network))
    );
    // `.pdl`: 22px rows 10px apart, 16px under the rule.
    for (y, label, value) in [
        (141, "Lock", text.lock),
        (173, "Seed phrase", text.backup),
        (205, "Recovery Kit", text.connect),
        (237, "Wallets", text.wallets),
    ] {
        let value: String = value.chars().take(34).collect();
        let _ = write!(
            svg,
            "<text x=\"28\" y=\"{y}\" font-size=\"13\" fill=\"{muted}\">{label}</text><text x=\"124\" y=\"{y}\" font-size=\"14\" fill=\"{ink}\">{}</text>",
            xml_text(&value)
        );
    }
    // `.pact`: Open, then the Back and Delete… links with accent underlines.
    let _ = write!(
        svg,
        "<rect x=\"28\" y=\"330.5\" width=\"66.5\" height=\"36\" rx=\"8\" fill=\"#f3821d\"/><text x=\"61.25\" y=\"353\" text-anchor=\"middle\" font-size=\"14\" font-weight=\"500\" fill=\"#201407\">Open</text><text x=\"114.5\" y=\"352\" font-size=\"14\" fill=\"{ink}\">Back</text><rect x=\"114.5\" y=\"358\" width=\"32\" height=\"1\" fill=\"{accent}\"/>"
    );
    let _ = write!(
        svg,
        "<text x=\"170.5\" y=\"352\" font-size=\"14\" fill=\"{ink}\">Delete…</text><rect x=\"170.5\" y=\"358\" width=\"50\" height=\"1\" fill=\"{accent}\"/>"
    );
    let _ = write!(
        svg,
        "<text x=\"28\" y=\"389\" font-size=\"12\" fill=\"{muted}\">Next step · {}</text></svg>",
        if text.passkey {
            "Passkey prompt"
        } else {
            "PIN prompt"
        }
    );
    svg
}

/// The unlock face as the PIN or passkey screen lays it out.
pub(super) fn unlock_svg(network: ChainId, light: bool, name: &str, passkey: bool) -> String {
    let ink = hex(Palette::new(light).ink);
    let muted = if light { "#575a54" } else { "#b9bbb5" };
    let line = if light { "#c4beb4" } else { "#424242" };
    let field = if light { "#faf9f6" } else { "#161716" };
    let accent = if light { "#a14d00" } else { "#f3821d" };
    let name: String = name.chars().take(22).collect();
    // `.pn`: "Open <name>" at 20px semibold, then the `.pnet` line 8px
    // below it — the 18px coin and the network at 14px — as the live face
    // lays them out. Everything under it sits 28px lower to make room.
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 420 420\" width=\"420\" height=\"420\" font-family=\"IBM Plex Sans\" color=\"{ink}\"><text x=\"28\" y=\"48\" font-size=\"20\" font-weight=\"600\" fill=\"{ink}\">Open {}</text>{}<text x=\"52\" y=\"77\" font-size=\"14\" fill=\"{ink}\">{}</text>",
        xml_text(&name),
        launcher_coin::fragment(network, 28.0, 63.0, 18.0, 1.0, 1.5),
        xml_text(super::launcher_chrome::network_name(network))
    );
    if passkey {
        let _ = write!(
            svg,
            "<rect x=\"28\" y=\"106\" width=\"164\" height=\"36\" rx=\"8\" fill=\"#f3821d\"/><text x=\"110\" y=\"129\" text-anchor=\"middle\" font-size=\"14\" font-weight=\"500\" fill=\"#201407\">Unlock with passkey</text>"
        );
    } else {
        // `.fld`: the PIN legend, four 48px `.dg` boxes, `.pineye`, `.help`,
        // then Unlock at `opacity: .45` until all four digits are in.
        let _ = write!(
            svg,
            "<text x=\"28\" y=\"119\" font-size=\"13\" font-weight=\"500\" fill=\"{ink}\">PIN</text>"
        );
        for i in 0..4 {
            let _ = write!(
                svg,
                "<rect x=\"{}.5\" y=\"129.5\" width=\"47\" height=\"47\" rx=\"8\" fill=\"{field}\" stroke=\"{line}\"/>",
                28 + i * 58
            );
        }
        let _ = write!(
            svg,
            "<g transform=\"translate(274 143) scale(0.833)\"><path d=\"M1.5 12S5 5.5 12 5.5 22.5 12 22.5 12 19 18.5 12 18.5 1.5 12 1.5 12z\" fill=\"none\" stroke=\"{muted}\" stroke-width=\"1.8\"/><circle cx=\"12\" cy=\"12\" r=\"3\" fill=\"{muted}\"/></g><text x=\"28\" y=\"192\" font-size=\"12\" fill=\"{muted}\">Enter this Cube's PIN.</text><rect x=\"28\" y=\"217\" width=\"76\" height=\"36\" rx=\"8\" fill=\"#f3821d\" fill-opacity=\".45\"/><text x=\"66\" y=\"240\" text-anchor=\"middle\" font-size=\"14\" font-weight=\"500\" fill=\"#201407\" fill-opacity=\".45\">Unlock</text>"
        );
    }
    let _ = write!(
        svg,
        "<text x=\"32\" y=\"381\" font-size=\"14\" fill=\"{ink}\">Cancel</text><rect x=\"32\" y=\"388.5\" width=\"42\" height=\"1\" fill=\"{accent}\"/></svg>"
    );
    svg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str) -> CubeCatalogKey {
        CubeCatalogKey {
            chain: ChainId::Bitcoin,
            cube_id: id.into(),
        }
    }

    fn visible(faces: &[ProjectedFace]) -> Vec<Face> {
        faces.iter().map(|f| f.face).collect()
    }

    #[test]
    fn resting_pose_shows_top_left_and_right_like_css() {
        let layout = Layout::new(DESIGN_STAGE_WIDTH, DESIGN_STAGE_HEIGHT);
        let rest = layout.rest_center(0, 4);
        let (_, faces) = project(&layout, rest, Pose::REST);
        assert_eq!(visible(&faces), vec![Face::Top, Face::Left, Face::Right]);
        let top = faces.iter().find(|f| f.face == Face::Top).unwrap();
        // rotateX(-35deg) rotateY(45deg): a diamond whose near corner is
        // lower than its far corner, centered on the Cube.
        let ys: Vec<f32> = top.corners.iter().map(|c| c.1).collect();
        assert!(ys.iter().all(|y| *y < rest.1));
        let left = top.corners.iter().map(|c| c.0).fold(f32::MAX, f32::min);
        let right = top.corners.iter().map(|c| c.0).fold(f32::MIN, f32::max);
        assert!(((left + right) / 2.0 - rest.0).abs() < 0.5);
        let _ = (left, right);
        // The prototype's 152px Cube renders about 224px wide at rest: the
        // side corners sit off the z = 0 plane after rotateX(-35deg).
        let xs: Vec<f32> = faces
            .iter()
            .flat_map(|f| f.corners.iter().map(|c| c.0))
            .collect();
        let width = xs.iter().cloned().fold(f32::MIN, f32::max)
            - xs.iter().cloned().fold(f32::MAX, f32::min);
        assert!((width - 224.0).abs() < 4.0, "{}", width);
    }

    #[test]
    fn details_and_unlock_faces_are_square_420px_on_the_optical_axis() {
        let layout = Layout::new(DESIGN_STAGE_WIDTH, DESIGN_STAGE_HEIGHT);
        let rest = layout.rest_center(3, 4);
        for (pose, face) in [
            (Pose::DETAILS, Face::BackLeft),
            (Pose::unlock(1.0), Face::BackRight),
        ] {
            let (frame, faces) = project(&layout, rest, pose);
            assert_eq!(visible(&faces), vec![face]);
            let c = faces[0].corners;
            assert!((c[1].0 - c[0].0 - FACE_SIZE).abs() < 0.05);
            assert!((c[3].1 - c[0].1 - FACE_SIZE).abs() < 0.05);
            assert!((c[0].0 - (720.0 - 210.0)).abs() < 0.05);
            // Centered on the page arrows' line, the resting Cubes' center.
            assert!((c[0].1 - (layout.horizon() - 210.0)).abs() < 0.05);
            assert_eq!(frame.shadow_opacity, 0.0);
        }
    }

    #[test]
    fn spin_uses_css_easing_handoff_delay_and_peer_dimming() {
        // x(u) = 0.5 at u = 0.66, where y(u) = 0.961.
        assert!((ease(0.5) - 0.961).abs() < 0.002, "{}", ease(0.5));
        let start = Instant::now();
        let a = key("a");
        let b = key("b");
        let c = key("c");
        let select = Motion::at(None, start);
        let mid = start + TURN / 2;
        assert!((select.pose(&a, Some(&a), mid).spin - ease(0.5)).abs() < 1e-6);
        let peer = select.pose(&b, Some(&a), start + PEERS);
        assert_eq!(peer.dim, 1.0);
        assert!(!select.done(start + TURN - Duration::from_millis(1)));
        assert!(select.done(start + TURN));

        let handoff = Motion::at(Some(a.clone()), start);
        let incoming = handoff.pose(&b, Some(&b), start + HANDOFF / 2);
        assert_eq!(incoming.spin, 0.0, "incoming waits 160ms");
        assert!(incoming.dim < 1.0, "incoming brightens at once");
        let outgoing = handoff.pose(&a, Some(&b), start + HANDOFF / 2);
        assert!(outgoing.spin < 1.0 && outgoing.dim > 0.0);
        assert_eq!(handoff.pose(&c, Some(&b), start).dim, 1.0);
        assert!(!handoff.done(start + TURN));
        assert!(handoff.done(start + TURN + HANDOFF));

        let back = Motion::returning_at(a.clone(), start);
        assert!((back.pose(&a, None, mid).spin - (1.0 - ease(0.5))).abs() < 1e-6);
        assert_eq!(back.pose(&b, None, start + PEERS).dim, 0.0);
        assert_eq!(settled_pose(&b, Some(&a)), Pose::DIMMED);
    }

    #[test]
    fn dimmed_scene_scales_about_its_center_and_fades_as_a_group() {
        let layout = Layout::new(DESIGN_STAGE_WIDTH, DESIGN_STAGE_HEIGHT);
        let rest = layout.rest_center(1, 4);
        let (lit, a) = project(&layout, rest, Pose::REST);
        let (dim, b) = project(&layout, rest, Pose::DIMMED);
        assert_eq!(lit.opacity, 1.0);
        assert!((dim.opacity - 0.4).abs() < 1e-6);
        assert!((b[0].size / a[0].size - 0.92).abs() < 1e-4);
    }

    #[test]
    fn narrow_windows_keep_four_cubes_inside_the_stage() {
        let layout = Layout::new(1024.0, 600.0);
        let first = layout.rest_center(0, 4);
        let last = layout.rest_center(3, 4);
        let half = layout.size() * 0.75;
        assert!(first.0 - half >= 0.0 && last.0 + half <= 1024.0);
        let (_, faces) = project(&layout, last, Pose::DETAILS);
        let c = faces[0].corners;
        assert!(c[0].1 >= 0.0 && c[3].1 <= 600.0);
    }

    #[test]
    fn face_art_escapes_names_and_follows_persisted_marks() {
        let marks = CubeMarks {
            network: ChainId::Testnet4,
            vault: true,
            imported_bitcoin: false,
            spark: false,
            passkey: false,
            backup: BackupMark::NotConfirmed,
            ghost: None,
        };
        let top = rest_face_svg(Face::Top, marks, false).unwrap();
        assert!(top.contains("rotate(45 "));
        assert!(top.contains(">Testnet4</text>"));
        assert!(top.contains("data:image/png;base64,"));
        let left = rest_face_svg(Face::Left, marks, false).unwrap();
        assert!(left.contains(SHIELD) && !left.contains(SPARK));
        let spark = rest_face_svg(
            Face::Left,
            CubeMarks {
                spark: true,
                ..marks
            },
            false,
        )
        .unwrap();
        assert!(spark.find(SHIELD) < spark.find(SPARK), "Vault, then Spark");
        let spark_only = CubeMarks {
            vault: false,
            spark: true,
            ..marks
        };
        let spark_only = rest_face_svg(Face::Left, spark_only, false).unwrap();
        assert!(
            spark_only.contains(&format!("<path d=\"{}\" fill=\"#", SPARK)),
            "{}",
            "Spark is the filled asterisk, not a stroked outline"
        );
        assert!(
            include_str!("../../../coincube-ui/static/logos/spark_badge.svg").contains(SPARK),
            "{}",
            "the face draws the same mark as the app's Spark badge"
        );
        let right = rest_face_svg(Face::Right, marks, true).unwrap();
        assert!(right.contains(LOCKED) && right.contains(ATTENTION));
        assert!(right.contains("#77450b"), "light warn tone");
        assert!(!right.contains(QUESTION));
        let unknown = CubeMarks {
            backup: BackupMark::Unknown,
            ..marks
        };
        let right = rest_face_svg(Face::Right, unknown, false).unwrap();
        assert!(right.contains(QUESTION) && right.contains("#b9bbb5"));
        assert!(!right.contains(ATTENTION));
        let unmarked = CubeMarks {
            vault: false,
            backup: BackupMark::Confirmed,
            passkey: true,
            network: ChainId::Bitcoin,
            ..marks
        };
        assert!(rest_face_svg(Face::Left, unmarked, false).is_none());
        let right = rest_face_svg(Face::Right, unmarked, false).unwrap();
        assert!(
            right.contains(PASSKEY) && !right.contains(CIRCLE),
            "no badge"
        );
        assert!(!rest_face_svg(Face::Top, unmarked, false)
            .unwrap()
            .contains("<text"));
        let details = details_svg(
            ChainId::Bitcoin,
            false,
            FaceText {
                name: "Alice & <Bob>",
                lock: "Locked",
                backup: "Recorded",
                connect: "Signed in",
                wallets: "Vault",
                passkey: false,
            },
        );
        assert!(details.contains("Alice &amp; &lt;Bob&gt;"));
        assert!(!details.contains("Alice & <Bob>"));
        assert!(unlock_svg(ChainId::Bitcoin, false, "A<B", false).contains("Open A&lt;B"));
    }

    #[test]
    fn cube_and_label_block_is_centered_with_arrows_on_the_cube_line() {
        for (width, height) in [(1024.0, 600.0), (1440.0, 742.0), (2000.0, 1100.0)] {
            let layout = Layout::new(width, height);
            let rest = layout.rest_center(0, 4);
            let (_, top, _, _) = bounds(&layout, rest, Pose::REST);
            let (_, _, _, foot) = label_rect(&layout, 0, 4);
            assert!(
                ((top + foot) / 2.0 - height / 2.0).abs() < 0.01,
                "block centered at {}x{}",
                width,
                height
            );
            assert_eq!(rest.1, layout.horizon());
            assert!(
                layout.horizon() < height / 2.0,
                "the Cube sits above the label"
            );
        }
    }

    #[test]
    fn hover_bob_rises_from_rest_and_lifts_the_cube_off_its_shadow() {
        assert_eq!(bob(Duration::ZERO), 0.0);
        assert!((bob(BOB_PERIOD / 2) - BOB_HEIGHT).abs() < 1e-4);
        assert!(bob(BOB_PERIOD) < 1e-4);
        assert!(bob(Duration::from_millis(16)) < 0.01, "no jump on enter");
        assert_eq!(settle(BOB_HEIGHT, Duration::ZERO), BOB_HEIGHT);
        assert_eq!(settle(BOB_HEIGHT, BOB_SETTLE), 0.0);

        let layout = Layout::new(DESIGN_STAGE_WIDTH, DESIGN_STAGE_HEIGHT);
        let rest = layout.rest_center(1, 4);
        let (still, a) = project(&layout, rest, Pose::REST);
        let (up, b) = project(&layout, rest, Pose::REST.lifted(BOB_HEIGHT));
        for (a, b) in a.iter().zip(&b) {
            for (p, q) in a.corners.iter().zip(&b.corners) {
                assert!((p.0 - q.0).abs() < 1e-3);
                assert!((p.1 - q.1 - BOB_HEIGHT).abs() < 1e-3);
            }
        }
        assert_eq!(up.shadow_center, still.shadow_center);
        assert!(up.shadow_radii.0 < still.shadow_radii.0);
        assert!(up.shadow_opacity < still.shadow_opacity);
    }

    #[test]
    fn ghost_faces_grey_the_coin_show_wallets_and_cloud_and_mark_the_kit() {
        for (ghost, mark, absent) in [
            (Ghost::Restorable, KIT, NOKIT),
            (Ghost::Unrestorable, NOKIT, KIT),
        ] {
            let marks = CubeMarks::ghost(ChainId::Bitcoin, ghost);
            let top = rest_face_svg(Face::Top, marks, false).unwrap();
            assert!(top.contains("feColorMatrix") && top.contains("opacity=\".55\""));
            // The cloud always; a Vault only once Connect reports one.
            let left = rest_face_svg(Face::Left, marks, false).unwrap();
            assert!(left.contains(CLOUD) && !left.contains(SHIELD));
            assert!(left.contains("#b9bbb5"), "muted, not ink");
            let vault = rest_face_svg(Face::Left, marks.with_vault(true), false).unwrap();
            assert!(vault.contains(SHIELD) && vault.contains(CLOUD));
            let right = rest_face_svg(Face::Right, marks, true).unwrap();
            assert!(right.contains(mark) && !right.contains(absent));
            assert!(!right.contains(LOCKED), "a ghost has no lock to show");
            // Only a ghost nothing restores carries the warning badge.
            assert_eq!(
                right.contains(ATTENTION),
                ghost == Ghost::Unrestorable,
                "{:?}",
                ghost
            );
        }
        assert!(rest_face_svg(
            Face::Right,
            CubeMarks::ghost(ChainId::Bitcoin, Ghost::Restorable),
            false
        )
        .unwrap()
        .contains("#8bdfab"));
        // Both ghost kinds share the edge; only the fill tells them apart.
        assert_eq!(ghost_fill(Ghost::Unrestorable, false)[3], 0);
        assert!(ghost_fill(Ghost::Restorable, false)[3] > 0);
        assert_eq!(ghost_edge(ChainId::Bitcoin, false), [243, 130, 29, 128]);
    }

    #[test]
    fn a_see_through_cube_keeps_its_far_faces_behind_the_near_ones() {
        let layout = Layout::new(1440.0, DESIGN_STAGE_HEIGHT);
        let rest = layout.rest_center(0, 1);
        let (_, solid) = project(&layout, rest, Pose::REST);
        let (_, ghost) = project_faces(&layout, rest, Pose::REST, true);
        assert_eq!(ghost.len(), Face::ALL.len());
        let near: Vec<_> = solid.iter().map(|f| f.face).collect();
        let tail: Vec<_> = ghost[ghost.len() - near.len()..]
            .iter()
            .map(|f| f.face)
            .collect();
        assert_eq!(tail, near, "the near faces paint last, unchanged");
        assert!(ghost[..ghost.len() - near.len()]
            .iter()
            .all(|f| f.area() <= 0.5));
    }

    #[test]
    fn ghost_details_snapshot_escapes_its_name_and_names_its_action() {
        for (restorable, action) in [
            (true, "Restore to this device"),
            (false, "Import with recovery phrase…"),
        ] {
            let svg = ghost_details_svg(
                ChainId::Bitcoin,
                false,
                GhostText {
                    name: "Old <laptop>",
                    backup: "No Recovery Kit in Connect",
                    restorable,
                },
            );
            assert!(svg.contains("Old &lt;laptop&gt;") && svg.contains(action));
            assert!(svg.contains("In Connect, not on this device"));
        }
    }
}
