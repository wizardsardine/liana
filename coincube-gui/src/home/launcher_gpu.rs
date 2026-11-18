//! GPU rasterizer for the Launcher floor.
//!
//! The previous floor serialized a full-stage SVG every 16ms tick and handed
//! it to resvg, which rasterized it on the CPU. Each frame was a new image
//! handle, so nothing was cached, and SVG has no notion of perspective: face
//! art could only be mapped affinely onto a face, and every border, shadow and
//! group opacity was approximated with strokes and flat ellipses.
//!
//! This renders the same scene the browser composes. Every visible face is a
//! quad with homogeneous coordinates from [`super::launcher_scene::project`],
//! so its artwork is perspective-correct. Each `.scene` is drawn with 4x MSAA
//! into its own layer and then composited with its group opacity, the floor
//! and all blending happen in sRGB space like CSS compositing, and the shadow
//! is the Gaussian `filter: blur()` of an ellipse. Text and controls stay Iced
//! widgets laid over the stage; rasterized face art is cached by content.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
};

use iced::{mouse, wgpu, widget::shader, Rectangle};

use super::launcher_scene::{
    self, border_fraction, details_svg, ghost_details_svg, rest_face_svg, unlock_svg, CubeMarks,
    Face, FaceText, GhostText, Layout, Palette, Pose, ProjectedFace, SceneFrame,
};
const MSAA: u32 = 4;
const LAYER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Face art is rasterized at roughly twice its largest on-screen size.
const REST_ART_PX: u32 = 512;
const PANEL_ART_PX: u32 = 840;

/// Rasterized face art in premultiplied sRGB bytes with a full mip chain.
#[derive(Debug)]
pub(super) struct Art {
    key: u64,
    levels: Vec<(u32, Vec<u8>)>,
}

fn fontdb() -> Arc<resvg::usvg::fontdb::Database> {
    static DB: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        for bytes in coincube_ui::font::load() {
            db.load_font_data(bytes.into_owned());
        }
        db.set_sans_serif_family("IBM Plex Sans");
        Arc::new(db)
    })
    .clone()
}

/// Retained CPU face art, in bytes. One page needs about 16MB: four Cubes'
/// 512px faces (1.4MB each with mips) and one 840px panel (3.8MB).
const ART_BUDGET: usize = 64 << 20;

impl Art {
    fn bytes(&self) -> usize {
        self.levels.iter().map(|(_, data)| data.len()).sum()
    }
}

/// Rasterize `svg` once per content and size. The cache is global across
/// pages, themes and status text, so it is bounded by bytes, not entries.
type ArtCache = Mutex<(HashMap<u64, Arc<Art>>, usize)>;

fn art_cache() -> &'static ArtCache {
    static CACHE: OnceLock<ArtCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new((HashMap::new(), 0)))
}

pub(super) fn art(svg: &str, px: u32) -> Option<Arc<Art>> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    svg.hash(&mut hasher);
    px.hash(&mut hasher);
    let key = hasher.finish();
    let cache = art_cache();
    if let Some(found) = cache.lock().ok()?.0.get(&key) {
        return Some(found.clone());
    }
    let options = resvg::usvg::Options {
        fontdb: fontdb(),
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_str(svg, &options).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(px, px)?;
    let size = tree.size();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(
            px as f32 / size.width(),
            px as f32 / size.height(),
        ),
        &mut pixmap.as_mut(),
    );
    let mut levels = vec![(px, pixmap.data().to_vec())];
    while levels.last().map_or(1, |(size, _)| *size) > 1 {
        let (size, data) = levels.last().unwrap();
        let (size, half) = (*size as usize, (*size as usize / 2).max(1));
        let mut next = vec![0u8; half * half * 4];
        for y in 0..half {
            for x in 0..half {
                for c in 0..4 {
                    let at = |dx: usize, dy: usize| {
                        let sx = (x * 2 + dx).min(size - 1);
                        let sy = (y * 2 + dy).min(size - 1);
                        data[(sy * size + sx) * 4 + c] as u32
                    };
                    next[(y * half + x) * 4 + c] =
                        ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8;
                }
            }
        }
        levels.push((half as u32, next));
    }
    let art = Arc::new(Art { key, levels });
    let mut cache = cache.lock().ok()?;
    let (entries, bytes) = &mut *cache;
    if *bytes + art.bytes() > ART_BUDGET {
        entries.clear();
        *bytes = 0;
    }
    *bytes += art.bytes();
    entries.insert(key, art.clone());
    debug_assert!(*bytes <= ART_BUDGET);
    Some(art)
}

/// One Cube on the stage, already posed.
pub(super) struct StageCube<'a> {
    pub rest: (f32, f32),
    pub pose: Pose,
    pub marks: CubeMarks,
    pub details: Option<FaceText<'a>>,
    /// A ghost's details face, in place of `details`.
    pub ghost_details: Option<GhostText<'a>>,
    pub unlock_name: Option<&'a str>,
    /// The loading face's edge light, on an opening Cube.
    pub glow: Option<Glow>,
}

/// `.glow-bright .f.bt::before` and `::after`: the light on the loading
/// face's edge, and on the List view's screen frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Glow {
    /// `--glow`, the network family hue.
    pub color: [u8; 3],
    /// `::before { opacity: var(--lvl) }`: the steady light.
    pub level: f32,
    /// `::after`: the ignition flicker, then the breath.
    pub pulse: f32,
}

/// One edge light: the shape's own corners (TL, TR, BR, BL), their `w`, and
/// its half extents and corner radius in its own pixels.
#[derive(Debug, Clone, Copy)]
struct GlowDraw {
    corners: [(f32, f32); 4],
    w: [f32; 4],
    half: (f32, f32),
    radius: f32,
    /// The light's own size: the loading face's edge scales with the Cube,
    /// as CSS scales the 152px Cube's border and shadows; the frame is 1:1.
    scale: f32,
    glow: Glow,
}

/// The widest halo, `0 0 34px` (sigma 17), fades out within this margin,
/// in the light's own pixels.
const GLOW_MARGIN: f32 = 56.0;

#[derive(Debug)]
struct FaceDraw {
    projected: ProjectedFace,
    color: [u8; 3],
    /// 1 for a solid face; a ghost's faint or empty fill below that.
    alpha: f32,
    art: Option<Arc<Art>>,
}

#[derive(Debug)]
struct SceneLayer {
    frame: SceneFrame,
    faces: Vec<FaceDraw>,
    edge: [u8; 4],
    dashed: bool,
    border: f32,
    /// A ghost's dashed footprint, `.gfoot`, and its opacity.
    footprint: Option<f32>,
    glow: Option<GlowDraw>,
}

/// Everything one frame of the stage needs, independent of the GPU.
#[derive(Debug)]
pub(super) struct StageData {
    width: f32,
    height: f32,
    /// The floor's back edge, on the resting Cubes' center line.
    horizon: f32,
    palette: Palette,
    floor: bool,
    layers: Vec<SceneLayer>,
}

/// `background: linear-gradient(rgba(glow, a), ...), base`, in sRGB.
fn mix(base: [u8; 3], over: [u8; 3], a: f32) -> [u8; 3] {
    let a = a.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| (base[i] as f32 + (over[i] as f32 - base[i] as f32) * a).round() as u8)
}

fn stage_face_art(
    cube: &StageCube<'_>,
    projected: &ProjectedFace,
    facing: bool,
    light: bool,
) -> Option<Arc<Art>> {
    let ghost = cube.marks.ghost;
    if ghost.is_some() && !facing {
        None
    } else {
        match projected.face {
            Face::Top | Face::Left | Face::Right => {
                rest_face_svg(projected.face, cube.marks, light)
                    .and_then(|svg| art(&svg, REST_ART_PX))
            }
            // The live details panel owns a square, settled face.
            Face::BackLeft if ghost.is_some() => cube
                .ghost_details
                .filter(|_| cube.pose.spin < 1.0)
                .and_then(|text| {
                    art(
                        &ghost_details_svg(cube.marks.network, light, text),
                        PANEL_ART_PX,
                    )
                }),
            Face::BackLeft => cube
                .details
                .filter(|_| cube.pose.spin < 1.0 || cube.pose.second > 0.0)
                .and_then(|text| art(&details_svg(cube.marks.network, light, text), PANEL_ART_PX)),
            Face::BackRight => cube
                .unlock_name
                .filter(|_| cube.pose.second < 1.0)
                .and_then(|name| {
                    art(
                        &unlock_svg(cube.marks.network, light, name, cube.marks.passkey),
                        PANEL_ART_PX,
                    )
                }),
            // The live loading content owns it.
            Face::Bottom => None,
        }
    }
}

impl StageData {
    /// The List view's opening: no Cubes, the glowing frame inset 20px from
    /// the stage with 14px corners, `.lframe`.
    pub fn frame(width: f32, height: f32, light: bool, glow: Glow) -> Self {
        const INSET: f32 = 20.0;
        let (x0, y0, x1, y1) = (INSET, INSET, width - INSET, height - INSET);
        Self {
            width,
            height,
            horizon: 0.0,
            palette: Palette::new(light),
            floor: false,
            layers: vec![SceneLayer {
                frame: SceneFrame {
                    opacity: 1.0,
                    shadow_center: (0.0, 0.0),
                    shadow_radii: (0.0, 0.0),
                    shadow_sigma: 0.0,
                    shadow_opacity: 0.0,
                },
                faces: Vec::new(),
                edge: [0; 4],
                dashed: false,
                border: 0.0,
                footprint: None,
                glow: Some(GlowDraw {
                    corners: [(x0, y0), (x1, y0), (x1, y1), (x0, y1)],
                    w: [1.0; 4],
                    half: ((x1 - x0) / 2.0, (y1 - y0) / 2.0),
                    radius: 14.0,
                    scale: 1.0,
                    glow,
                }),
            }],
        }
    }

    /// A presentation-only create affordance: no Cube identity, wallet facts,
    /// network art, recovery status or catalog entry is fabricated.
    /// `lift` is its hover float, as [`Pose::lifted`] takes.
    pub fn create_placeholder(layout: &Layout, light: bool, lift: f32) -> Self {
        let palette = Palette::new(light);
        let (frame, faces) = launcher_scene::project_faces(
            layout,
            layout.rest_center(0, 1),
            Pose::REST.lifted(lift),
            true,
        );
        Self {
            width: layout.width,
            height: layout.height,
            horizon: layout.horizon(),
            palette,
            floor: true,
            layers: vec![SceneLayer {
                frame: SceneFrame {
                    shadow_opacity: 0.0,
                    ..frame
                },
                faces: faces
                    .into_iter()
                    .map(|projected| FaceDraw {
                        color: palette.face(projected.face),
                        alpha: 0.06,
                        projected,
                        art: None,
                    })
                    .collect(),
                edge: [palette.ink[0], palette.ink[1], palette.ink[2], 150],
                dashed: true,
                border: border_fraction(layout.size()),
                footprint: None,
                glow: None,
            }],
        }
    }

    /// Pose every Cube and choose its face art. Callers pass Cubes in paint
    /// order: peers in page order, then the outgoing Cube, then the lead.
    pub fn new<'a>(
        layout: &Layout,
        light: bool,
        floor: bool,
        cubes: impl IntoIterator<Item = StageCube<'a>>,
    ) -> Self {
        let palette = Palette::new(light);
        let layers = cubes
            .into_iter()
            .map(|cube| {
                let ghost = cube.marks.ghost;
                let (frame, projected) =
                    launcher_scene::project_faces(layout, cube.rest, cube.pose, ghost.is_some());
                let spin = cube.pose.spin.clamp(0.0, 1.0);
                // A ghost has no `.shadow`, and rests at
                // `.cb.ghostcb:not(.dim):not(.active) .scene { opacity: .85 }`.
                let frame = match ghost {
                    Some(_) => SceneFrame {
                        shadow_opacity: 0.0,
                        opacity: frame.opacity
                            * (1.0 - 0.15 * (1.0 - cube.pose.dim.clamp(0.0, 1.0)) * (1.0 - spin)),
                        ..frame
                    },
                    None => frame,
                };
                let faces = projected
                    .into_iter()
                    .map(|projected| {
                        // A ghost's turned-away faces show only their edges.
                        let facing = projected.area() > 0.5;
                        // `.cube.ghost .f.panel`: the face a ghost turns to is
                        // solid once it faces the viewer, so its details read
                        // as on a local Cube. A ghost never turns to unlock.
                        let panel = projected.face == Face::BackLeft && facing && spin > 0.0;
                        let (color, alpha) = match ghost {
                            Some(ghost) if !panel => {
                                let [r, g, b, a] = launcher_scene::ghost_fill(ghost, light);
                                ([r, g, b], a as f32 / 255.0)
                            }
                            // `.glow-bright .f.bt.on`: the loading face warms
                            // by `--lvl * .09` of the glow with each stage.
                            _ if projected.face == Face::Bottom => (
                                cube.glow.map_or(palette.face(Face::Bottom), |glow| {
                                    mix(palette.face(Face::Bottom), glow.color, glow.level * 0.09)
                                }),
                                1.0,
                            ),
                            _ => (palette.face(projected.face), 1.0),
                        };
                        let art = stage_face_art(&cube, &projected, facing, light);
                        FaceDraw {
                            color,
                            alpha,
                            projected,
                            art,
                        }
                    })
                    .collect::<Vec<FaceDraw>>();
                let glow = cube.glow.and_then(|glow| {
                    let face = faces
                        .iter()
                        .find(|face| face.projected.face == Face::Bottom)?;
                    let [a, b, _, _] = face.projected.corners;
                    let half = ((b.0 - a.0).hypot(b.1 - a.1) / 2.0).max(1.0);
                    Some(GlowDraw {
                        corners: face.projected.corners,
                        w: face.projected.w,
                        half: (half, half),
                        radius: 0.0,
                        scale: 2.0 * half / launcher_scene::REST_SIZE,
                        glow,
                    })
                });
                SceneLayer {
                    glow,
                    frame,
                    faces,
                    edge: match ghost {
                        Some(_) => launcher_scene::ghost_edge(cube.marks.network, light),
                        None => launcher_scene::edge(cube.marks.network, light),
                    },
                    dashed: super::launcher_coin::is_test_network(cube.marks.network),
                    border: border_fraction(layout.size()),
                    // `.gfoot { transition: opacity 500ms }`: gone once turned.
                    footprint: ghost.map(|_| 1.0 - spin),
                }
            })
            .collect();
        Self {
            width: layout.width,
            height: layout.height,
            horizon: layout.horizon(),
            palette,
            floor,
            layers,
        }
    }

    /// The faces this frame paints, for tests and diagnostics.
    #[cfg(test)]
    pub fn visible(&self) -> Vec<Vec<Face>> {
        self.layers
            .iter()
            .map(|layer| layer.faces.iter().map(|f| f.projected.face).collect())
            .collect()
    }

    #[cfg(test)]
    pub fn opacities(&self) -> Vec<f32> {
        self.layers
            .iter()
            .map(|layer| layer.frame.opacity)
            .collect()
    }

    #[cfg(test)]
    pub fn has_art(&self, layer: usize, face: Face) -> bool {
        self.layers[layer]
            .faces
            .iter()
            .any(|f| f.projected.face == face && f.art.is_some())
    }
}

/// The Iced program: one primitive per draw of the stage.
pub(super) struct Stage(pub Arc<StageData>);

impl<Message> shader::Program<Message> for Stage {
    type State = ();
    type Primitive = StagePrimitive;

    fn draw(&self, _state: &(), _cursor: mouse::Cursor, _bounds: Rectangle) -> StagePrimitive {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        StagePrimitive {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            data: self.0.clone(),
        }
    }
}

/// A stage element: the shader program over a fixed-size stage.
pub(super) fn stage<'a, Message: 'a>(
    data: StageData,
) -> iced::Element<'a, Message, coincube_ui::theme::Theme> {
    let (width, height) = (data.width, data.height);
    iced::widget::Shader::new(Stage(Arc::new(data)))
        .width(iced::Length::Fixed(width))
        .height(iced::Length::Fixed(height))
        .into()
}

#[derive(Debug)]
pub(super) struct StagePrimitive {
    id: u64,
    data: Arc<StageData>,
}

fn srgb(c: [u8; 3]) -> [f32; 3] {
    c.map(|v| v as f32 / 255.0)
}

/// Vertex: clip position (x w, y w, 0, w), uv, color, edge, p0, p1.
const FLOATS: usize = 22;

#[derive(Default)]
struct Vertices(Vec<f32>);

impl Vertices {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        (x, y): (f32, f32),
        w: f32,
        size: (f32, f32),
        uv: (f32, f32),
        color: [f32; 4],
        edge: [f32; 4],
        p0: [f32; 4],
        p1: [f32; 4],
    ) {
        let nx = x / size.0 * 2.0 - 1.0;
        let ny = 1.0 - y / size.1 * 2.0;
        self.0
            .extend_from_slice(&[nx * w, ny * w, 0.0, w, uv.0, uv.1]);
        self.0.extend_from_slice(&color);
        self.0.extend_from_slice(&edge);
        self.0.extend_from_slice(&p0);
        self.0.extend_from_slice(&p1);
    }

    fn count(&self) -> u32 {
        (self.0.len() / FLOATS) as u32
    }

    /// Two triangles over `quad` (TL, TR, BR, BL) with matching data.
    #[allow(clippy::too_many_arguments)]
    fn quad(
        &mut self,
        quad: [(f32, f32); 4],
        w: [f32; 4],
        uv: [(f32, f32); 4],
        size: (f32, f32),
        color: [f32; 4],
        edge: [f32; 4],
        p0: [f32; 4],
        p1: [f32; 4],
    ) {
        for i in [0, 1, 2, 0, 2, 3] {
            self.push(quad[i], w[i], size, uv[i], color, edge, p0, p1);
        }
    }
}

struct Draw {
    range: std::ops::Range<u32>,
    art: Option<Arc<Art>>,
}

struct PreparedLayer {
    draws: Vec<Draw>,
    opacity: f32,
    /// Physical texels this scene can touch: its passes are scissored to it.
    rect: (u32, u32, u32, u32),
}

struct Prepared {
    vertices: wgpu::Buffer,
    composite: wgpu::Buffer,
    floor: std::ops::Range<u32>,
    layers: Vec<PreparedLayer>,
    background: [f32; 3],
    /// Signed physical origin of the stage in the target.
    origin: (f32, f32),
    /// The stage's size in texels.
    size: (u32, u32),
}

struct Layers {
    size: (u32, u32),
    msaa: wgpu::TextureView,
    scratch: wgpu::BindGroup,
    scratch_view: wgpu::TextureView,
    base: wgpu::BindGroup,
    base_view: wgpu::TextureView,
}

struct ArtTexture {
    bind_group: wgpu::BindGroup,
    _texture: wgpu::Texture,
    used: u64,
}

pub(super) struct Pipeline {
    scene: wgpu::RenderPipeline,
    compose: wgpu::RenderPipeline,
    present: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    blank: wgpu::BindGroup,
    linearize: bool,
    layers: Option<Layers>,
    art: HashMap<u64, ArtTexture>,
    prepared: HashMap<u64, Prepared>,
    frame: u64,
}

const SHADER: &str = r#"
struct VsIn {
    @location(0) pos: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) edge: vec4<f32>,
    @location(4) p0: vec4<f32>,
    @location(5) p1: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) edge: vec4<f32>,
    @location(3) p0: vec4<f32>,
    @location(4) p1: vec4<f32>,
};

@group(0) @binding(0) var art: texture_2d<f32>;
@group(0) @binding(1) var art_sampler: sampler;

@vertex
fn vs_scene(v: VsIn) -> VsOut {
    var out: VsOut;
    out.pos = v.pos;
    out.uv = v.uv;
    out.color = v.color;
    out.edge = v.edge;
    out.p0 = v.p0;
    out.p1 = v.p1;
    return out;
}

fn erf(x: f32) -> f32 {
    let a = 0.147;
    let x2 = x * x;
    let r = sqrt(1.0 - exp(-x2 * (1.2732395 + a * x2) / (1.0 + a * x2)));
    return select(-r, r, x >= 0.0);
}

// A blurred step: `box-shadow` of a large box, `sd` pixels outside it.
fn blurred(sd: f32, blur: f32) -> f32 {
    return 0.5 * (1.0 - erf(sd / (0.5 * blur * 1.4142135)));
}

// One `::before` or `::after` of `.glow-bright .f.bt`: a 2px border just
// outside the shape, three outer shadows and one inset, by blur radius.
fn edge_light(d: f32, b: vec4<f32>, a: vec4<f32>, inset: vec2<f32>) -> f32 {
    let line = clamp(d + 0.5, 0.0, 1.0) * clamp(2.5 - d, 0.0, 1.0);
    let out = d - 2.0;
    let o1 = select(0.0, a.x * blurred(out, b.x), out > 0.0);
    let o2 = select(0.0, a.y * blurred(out, b.y), out > 0.0);
    let o3 = select(0.0, a.z * blurred(out, b.z), out > 0.0);
    let i1 = select(0.0, inset.y * blurred(-d, inset.x), d < 0.0);
    return 1.0 - (1.0 - line) * (1.0 - o1) * (1.0 - o2) * (1.0 - o3) * (1.0 - i1);
}

@fragment
fn fs_scene(v: VsOut) -> @location(0) vec4<f32> {
    let kind = v.p0.w;
    if (kind > 3.5) {
        // The loading face's edge light: the signed distance from the
        // rounded box (half extents p1.xy, radius p1.z), then the steady
        // `::before` at `--lvl` and the flickering `::after`, combined.
        let r = v.p1.z;
        let q = abs(v.uv) - (v.p1.xy - vec2<f32>(r));
        let d = (length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r) / max(v.p0.z, 1e-3);
        let before = edge_light(d, vec4<f32>(2.0, 8.0, 22.0, 0.0), vec4<f32>(1.0, 0.75, 0.4, 0.0), vec2<f32>(12.0, 0.35));
        let after = edge_light(d, vec4<f32>(3.0, 12.0, 34.0, 0.0), vec4<f32>(1.0, 0.85, 0.5, 0.0), vec2<f32>(20.0, 0.45));
        let a = 1.0 - (1.0 - clamp(v.p0.x, 0.0, 1.0) * before) * (1.0 - clamp(v.p0.y, 0.0, 1.0) * after);
        return vec4<f32>(v.color.rgb * a, a);
    }
    if (kind > 2.5) {
        // A 1px dashed ellipse outline: `border: 1px dashed`.
        let r = v.p1.zw;
        let p = v.uv;
        let d = length(p / r);
        let g = length(p / (r * r)) / max(d, 1e-4);
        let sd = (d - 1.0) / max(g, 1e-4);
        let along = atan2(p.y / r.y, p.x / r.x) / 6.2831853 + 0.5;
        let dash = step(fract(along * v.p1.x), 0.5);
        let a = clamp(1.0 - abs(sd), 0.0, 1.0) * dash * v.color.a;
        return vec4<f32>(v.color.rgb * a, a);
    }
    if (kind > 1.5) {
        // Flat color: floor plane and its top rule.
        return vec4<f32>(v.color.rgb * v.color.a, v.color.a);
    }
    if (kind > 0.5) {
        // `filter: blur(sigma)` of an ellipse: the Gaussian-smoothed edge of
        // its signed distance, in pixels.
        let r = v.p1.zw;
        let p = v.uv;
        let d = length(p / r);
        let g = length(p / (r * r)) / max(d, 1e-4);
        let sd = (d - 1.0) / max(g, 1e-4);
        let a = 0.5 * (1.0 - erf(sd / (v.p1.x * 1.4142135))) * v.color.a;
        return vec4<f32>(v.color.rgb * a, a);
    }
    // Face: background, art over it, then the inset border, all in sRGB and
    // premultiplied. A solid face has `color.a` 1; a ghost's is faint or 0.
    var c = v.color.rgb * v.color.a;
    var a = v.color.a;
    if (v.p1.y > 0.5) {
        let t = textureSample(art, art_sampler, v.uv);
        c = t.rgb + c * (1.0 - t.a);
        a = t.a + a * (1.0 - t.a);
    }
    let du = min(v.uv.x, 1.0 - v.uv.x);
    let dv = min(v.uv.y, 1.0 - v.uv.y);
    let fu = max(fwidth(v.uv.x), 1e-5);
    let fv = max(fwidth(v.uv.y), 1e-5);
    let inside_u = clamp((du - v.p0.x) / fu + 0.5, 0.0, 1.0);
    let inside_v = clamp((dv - v.p0.y) / fv + 0.5, 0.0, 1.0);
    var border = 1.0 - min(inside_u, inside_v);
    if (v.p0.z > 0.0) {
        // `border-style: dashed` along each edge: across the top and bottom
        // edges the dashes run with `u`, down the left and right with `v`.
        // Near the top or bottom `du / fu > dv / fv`, so `select` picks `u`.
        let along = select(v.uv.y, v.uv.x, du / fu > dv / fv);
        border = border * step(fract(along * v.p0.z), 0.5);
    }
    let e = v.edge.a * border;
    return vec4<f32>(v.edge.rgb * e + c * (1.0 - e), e + a * (1.0 - e));
}

struct CompositeIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) opacity: f32,
    @location(3) linearize: f32,
};

struct CompositeOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) opacity: f32,
    @location(2) linearize: f32,
};

@vertex
fn vs_composite(v: CompositeIn) -> CompositeOut {
    var out: CompositeOut;
    out.pos = vec4<f32>(v.pos, 0.0, 1.0);
    out.uv = v.uv;
    out.opacity = v.opacity;
    out.linearize = v.linearize;
    return out;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_composite(v: CompositeOut) -> @location(0) vec4<f32> {
    let t = textureSample(art, art_sampler, v.uv / vec2<f32>(textureDimensions(art, 0)));
    if (v.linearize > 0.5) {
        return vec4<f32>(to_linear(t.rgb), 1.0);
    }
    return t * v.opacity;
}
"#;

fn texture_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("tenshu.launcher.texture"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("tenshu.launcher.bind"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn texture(
    device: &wgpu::Device,
    (width, height): (u32, u32),
    samples: u32,
    mips: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("tenshu.launcher.layer"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format: LAYER_FORMAT,
        usage,
        view_formats: &[],
    })
}

fn premultiplied() -> Option<wgpu::BlendState> {
    Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING)
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tenshu.launcher.shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let texture_layout = texture_layout(device);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tenshu.launcher.layout"),
            bind_group_layouts: &[&texture_layout],
            push_constant_ranges: &[],
        });
        let scene_attributes = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4
        ];
        let composite_attributes =
            wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32, 3 => Float32];
        let pipeline = |entry: (&str, &str),
                        attributes: &[wgpu::VertexAttribute],
                        stride: usize,
                        target: wgpu::TextureFormat,
                        samples: u32,
                        blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("tenshu.launcher.pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(entry.0),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: (stride * 4) as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes,
                    }],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry.1),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            })
        };
        let scene = pipeline(
            ("vs_scene", "fs_scene"),
            &scene_attributes,
            FLOATS,
            LAYER_FORMAT,
            MSAA,
            premultiplied(),
        );
        let compose = pipeline(
            ("vs_composite", "fs_composite"),
            &composite_attributes,
            6,
            LAYER_FORMAT,
            1,
            premultiplied(),
        );
        let present = pipeline(
            ("vs_composite", "fs_composite"),
            &composite_attributes,
            6,
            format,
            1,
            None,
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("tenshu.launcher.sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let blank_texture = texture(
            device,
            (1, 1),
            1,
            1,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &blank_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[0, 0, 0, 0],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let blank = bind(
            device,
            &texture_layout,
            &blank_texture.create_view(&Default::default()),
            &sampler,
        );
        Self {
            scene,
            compose,
            present,
            texture_layout,
            sampler,
            blank,
            linearize: format.is_srgb(),
            layers: None,
            art: HashMap::new(),
            prepared: HashMap::new(),
            frame: 0,
        }
    }

    fn trim(&mut self) {
        self.prepared.clear();
        self.frame += 1;
        let frame = self.frame;
        self.art
            .retain(|_, texture| frame.saturating_sub(texture.used) < 240);
    }
}

impl Pipeline {
    fn layers(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self
            .layers
            .as_ref()
            .is_some_and(|l| l.size.0 >= size.0 && l.size.1 >= size.1)
        {
            return;
        }
        let size = (
            size.0.max(self.layers.as_ref().map_or(0, |l| l.size.0)),
            size.1.max(self.layers.as_ref().map_or(0, |l| l.size.1)),
        );
        let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let sampled = attachment | wgpu::TextureUsages::TEXTURE_BINDING;
        let msaa = texture(device, size, MSAA, 1, attachment).create_view(&Default::default());
        let scratch_view = texture(device, size, 1, 1, sampled).create_view(&Default::default());
        let base_view = texture(device, size, 1, 1, sampled).create_view(&Default::default());
        self.layers = Some(Layers {
            size,
            msaa,
            scratch: bind(device, &self.texture_layout, &scratch_view, &self.sampler),
            scratch_view,
            base: bind(device, &self.texture_layout, &base_view, &self.sampler),
            base_view,
        });
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, art: &Art) {
        let frame = self.frame;
        if let Some(found) = self.art.get_mut(&art.key) {
            found.used = frame;
            return;
        }
        let size = art.levels[0].0;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tenshu.launcher.art"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: art.levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: LAYER_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, (size, data)) in art.levels.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * 4),
                    rows_per_image: Some(*size),
                },
                wgpu::Extent3d {
                    width: *size,
                    height: *size,
                    depth_or_array_layers: 1,
                },
            );
        }
        let bind_group = bind(
            device,
            &self.texture_layout,
            &texture.create_view(&Default::default()),
            &self.sampler,
        );
        self.art.insert(
            art.key,
            ArtTexture {
                bind_group,
                _texture: texture,
                used: frame,
            },
        );
    }
}

/// The part of the stage inside iced's clip, in the target's texels.
fn present_scissor(
    clip: &Rectangle<u32>,
    (ox, oy): (f32, f32),
    (width, height): (u32, u32),
) -> Option<(u32, u32, u32, u32)> {
    let (ox, oy) = (ox.floor() as i64, oy.floor() as i64);
    let x0 = (clip.x as i64).max(ox).max(0);
    let y0 = (clip.y as i64).max(oy).max(0);
    let x1 = (clip.x as i64 + clip.width as i64).min(ox + width as i64);
    let y1 = (clip.y as i64 + clip.height as i64).min(oy + height as i64);
    (x1 > x0 && y1 > y0).then(|| (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

fn pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    color: wgpu::RenderPassColorAttachment<'_>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("tenshu.launcher.pass"),
        color_attachments: &[Some(color)],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    })
}

fn buffer(device: &wgpu::Device, data: &[f32]) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("tenshu.launcher.vertices"),
        contents: if bytes.is_empty() { &[0; 4] } else { &bytes },
        usage: wgpu::BufferUsages::VERTEX,
    })
}

fn prepare_scene_layer(
    pipeline: &mut Pipeline,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    v: &mut Vertices,
    data: &StageData,
    layer: &SceneLayer,
    size_px: (u32, u32),
) -> PreparedLayer {
    let size = (data.width, data.height);
    let palette = data.palette;
    let none = [0.0; 4];
    let kind = |k: f32| [0.0, 0.0, 0.0, k];
    let first = v.0.len();
    let mut draws = Vec::new();
    let frame = layer.frame;
    if frame.shadow_opacity > 0.0 {
        let start = v.count();
        let (cx, cy) = frame.shadow_center;
        let (rx, ry) = frame.shadow_radii;
        let pad = frame.shadow_sigma * 3.0;
        let (hx, hy) = (rx + pad, ry + pad);
        let s = palette.shadow;
        v.quad(
            [
                (cx - hx, cy - hy),
                (cx + hx, cy - hy),
                (cx + hx, cy + hy),
                (cx - hx, cy + hy),
            ],
            [1.0; 4],
            [(-hx, -hy), (hx, -hy), (hx, hy), (-hx, hy)],
            size,
            [
                s[0] as f32 / 255.0,
                s[1] as f32 / 255.0,
                s[2] as f32 / 255.0,
                s[3] as f32 / 255.0 * frame.shadow_opacity,
            ],
            none,
            kind(1.0),
            [frame.shadow_sigma.max(0.01), 0.0, rx, ry],
        );
        draws.push(Draw {
            range: start..v.count(),
            art: None,
        });
    }
    if let Some(opacity) = layer.footprint.filter(|o| *o > 0.0) {
        // `.gfoot`: a dashed ellipse in `--line` where the shadow would
        // fall, 1.3s by .5s against the shadow's 1.45s by .62s.
        let start = v.count();
        let (cx, cy) = frame.shadow_center;
        let (rx, ry) = (
            frame.shadow_radii.0 * 0.65 / 0.725,
            frame.shadow_radii.1 * 0.25 / 0.31,
        );
        let (hx, hy) = (rx + 2.0, ry + 2.0);
        let line = srgb(palette.line);
        v.quad(
            [
                (cx - hx, cy - hy),
                (cx + hx, cy - hy),
                (cx + hx, cy + hy),
                (cx - hx, cy + hy),
            ],
            [1.0; 4],
            [(-hx, -hy), (hx, -hy), (hx, hy), (-hx, hy)],
            size,
            [line[0], line[1], line[2], opacity],
            none,
            kind(3.0),
            // About 4px dashes around the ring.
            [
                ((rx + ry) * std::f32::consts::PI / 8.0).round().max(4.0),
                0.0,
                rx,
                ry,
            ],
        );
        draws.push(Draw {
            range: start..v.count(),
            art: None,
        });
    }
    let edge = layer.edge;
    let edge = [
        edge[0] as f32 / 255.0,
        edge[1] as f32 / 255.0,
        edge[2] as f32 / 255.0,
        edge[3] as f32 / 255.0,
    ];
    // About 3px dashes on a 152px face, as Chromium draws 1.5px.
    let dashes = if layer.dashed {
        (1.0 / layer.border / 4.0).round()
    } else {
        0.0
    };
    for face in &layer.faces {
        let start = v.count();
        let color = srgb(face.color);
        v.quad(
            face.projected.corners,
            face.projected.w,
            [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
            size,
            [color[0], color[1], color[2], face.alpha],
            edge,
            [layer.border, layer.border, dashes, 0.0],
            [dashes, if face.art.is_some() { 1.0 } else { 0.0 }, 0.0, 0.0],
        );
        if let Some(art) = &face.art {
            pipeline.upload(device, queue, art);
        }
        draws.push(Draw {
            range: start..v.count(),
            art: face.art.clone(),
        });
    }
    if let Some(glow) = layer.glow {
        // The shape grown by the halo margin along its own edges, with
        // UVs in its own pixels from its centre.
        let start = v.count();
        let [c0, c1, _, c3] = glow.corners;
        let unit = |(x, y): (f32, f32)| {
            let n = x.hypot(y).max(1e-3);
            (x / n, y / n)
        };
        let along = unit((c1.0 - c0.0, c1.1 - c0.1));
        let down = unit((c3.0 - c0.0, c3.1 - c0.1));
        let signs = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        let margin = GLOW_MARGIN * glow.scale;
        let (hx, hy) = (glow.half.0 + margin, glow.half.1 + margin);
        let corners = [0, 1, 2, 3].map(|i| {
            let (sx, sy): (f32, f32) = signs[i];
            let (x, y) = glow.corners[i];
            (
                x + (along.0 * sx + down.0 * sy) * margin,
                y + (along.1 * sx + down.1 * sy) * margin,
            )
        });
        let color = srgb(glow.glow.color);
        v.quad(
            corners,
            glow.w,
            signs.map(|(sx, sy)| (sx * hx, sy * hy)),
            size,
            [color[0], color[1], color[2], 1.0],
            none,
            [glow.glow.level, glow.glow.pulse, glow.scale, 4.0],
            [glow.half.0, glow.half.1, glow.radius, 0.0],
        );
        draws.push(Draw {
            range: start..v.count(),
            art: None,
        });
    }
    // Bounds of this layer's vertices, back in stage pixels.
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for vertex in v.0[first..].chunks(FLOATS) {
        let (x, y) = (
            (vertex[0] / vertex[3] + 1.0) / 2.0 * size.0,
            (1.0 - vertex[1] / vertex[3]) / 2.0 * size.1,
        );
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let (sx, sy) = (size_px.0 as f32 / size.0, size_px.1 as f32 / size.1);
    let clamp = |v: f32, max: u32| (v.max(0.0) as u32).min(max);
    let (px0, py0) = (
        clamp((x0 * sx).floor() - 2.0, size_px.0),
        clamp((y0 * sy).floor() - 2.0, size_px.1),
    );
    let (px1, py1) = (
        clamp((x1 * sx).ceil() + 2.0, size_px.0),
        clamp((y1 * sy).ceil() + 2.0, size_px.1),
    );
    PreparedLayer {
        draws,
        opacity: frame.opacity,
        rect: (px0, py0, px1.saturating_sub(px0), py1.saturating_sub(py0)),
    }
}

impl shader::Primitive for StagePrimitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        let scale = viewport.scale_factor();
        // The origin stays signed: a scrolled stage starts above or left of
        // the frame, and its picture must stay under its Iced overlays.
        let origin = ((bounds.x * scale).round(), (bounds.y * scale).round());
        let physical = (
            0u32,
            0u32,
            ((bounds.width * scale).round() as u32).max(1),
            ((bounds.height * scale).round() as u32).max(1),
        );
        pipeline.layers(device, (physical.2, physical.3));
        let data = &self.data;
        let size = (data.width, data.height);
        let mut v = Vertices::default();
        let palette = data.palette;
        let flat = |c: [u8; 3], a: f32| {
            let c = srgb(c);
            [c[0], c[1], c[2], a]
        };
        let none = [0.0; 4];
        let kind = |k: f32| [0.0, 0.0, 0.0, k];
        if data.floor {
            // `.floor`: from the Cubes' line, inset 14% at the top, full
            // width at the bottom, with its 1px top rule between the insets.
            let top = data.horizon;
            let inset = data.width * launcher_scene::FLOOR_INSET;
            let floor = [
                (inset, top),
                (data.width - inset, top),
                (data.width, data.height),
                (0.0, data.height),
            ];
            v.quad(
                floor,
                [1.0; 4],
                [(0.0, 0.0); 4],
                size,
                flat(palette.floor, 1.0),
                none,
                kind(2.0),
                none,
            );
            let rule = [
                (inset, top),
                (data.width - inset, top),
                (data.width - inset, top + 1.0),
                (inset, top + 1.0),
            ];
            v.quad(
                rule,
                [1.0; 4],
                [(0.0, 0.0); 4],
                size,
                flat(palette.floor_rule, 1.0),
                none,
                kind(2.0),
                none,
            );
        }
        let floor_range = 0..v.count();
        let mut layers = Vec::with_capacity(data.layers.len());
        for layer in &data.layers {
            layers.push(prepare_scene_layer(
                pipeline,
                device,
                queue,
                &mut v,
                data,
                layer,
                (physical.2, physical.3),
            ));
        }
        // Composite triangles: one per layer into the base, then the base
        // into the frame. Each samples only the stage's corner of the layer.
        // UVs are in texels and normalized by the live texture size in the
        // shader, so a larger stage later in the frame cannot stale them.
        let (u, w) = (physical.2 as f32, physical.3 as f32);
        let mut composite = Vec::new();
        let mut triangle = |opacity: f32, linearize: f32| {
            for (x, y, s, t) in [
                (-1.0, 1.0, 0.0, 0.0),
                (3.0, 1.0, 2.0 * u, 0.0),
                (-1.0, -3.0, 0.0, 2.0 * w),
            ] {
                composite.extend_from_slice(&[x, y, s, t, opacity, linearize]);
            }
        };
        for layer in &layers {
            triangle(layer.opacity, 0.0);
        }
        triangle(1.0, if pipeline.linearize { 1.0 } else { 0.0 });
        pipeline.prepared.insert(
            self.id,
            Prepared {
                vertices: buffer(device, &v.0),
                composite: buffer(device, &composite),
                floor: floor_range,
                layers,
                background: srgb(palette.background),
                origin,
                size: (physical.2, physical.3),
            },
        );
    }

    fn render(
        &self,
        pipeline: &Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let (Some(prepared), Some(layers)) =
            (pipeline.prepared.get(&self.id), pipeline.layers.as_ref())
        else {
            return;
        };
        let (ox, oy) = prepared.origin;
        let (bw, bh) = prepared.size;
        let viewport = |pass: &mut wgpu::RenderPass<'_>| {
            pass.set_viewport(0.0, 0.0, bw as f32, bh as f32, 0.0, 1.0);
            pass.set_scissor_rect(0, 0, bw.min(layers.size.0), bh.min(layers.size.1));
        };
        let attachment = |view, resolve, load| wgpu::RenderPassColorAttachment {
            view,
            resolve_target: resolve,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
            depth_slice: None,
        };
        let [r, g, b] = prepared.background;
        {
            let mut base = pass(
                encoder,
                attachment(
                    &layers.msaa,
                    Some(&layers.base_view),
                    wgpu::LoadOp::Clear(wgpu::Color {
                        r: r as f64,
                        g: g as f64,
                        b: b as f64,
                        a: 1.0,
                    }),
                ),
            );
            viewport(&mut base);
            base.set_pipeline(&pipeline.scene);
            base.set_vertex_buffer(0, prepared.vertices.slice(..));
            base.set_bind_group(0, &pipeline.blank, &[]);
            base.draw(prepared.floor.clone(), 0..1);
        }
        for (index, layer) in prepared.layers.iter().enumerate() {
            let (rx, ry, rw, rh) = layer.rect;
            if rw == 0 || rh == 0 {
                continue;
            }
            {
                let mut scene = pass(
                    encoder,
                    attachment(
                        &layers.msaa,
                        Some(&layers.scratch_view),
                        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    ),
                );
                viewport(&mut scene);
                scene.set_scissor_rect(rx, ry, rw, rh);
                scene.set_pipeline(&pipeline.scene);
                scene.set_vertex_buffer(0, prepared.vertices.slice(..));
                for draw in &layer.draws {
                    let group = draw
                        .art
                        .as_ref()
                        .and_then(|art| pipeline.art.get(&art.key))
                        .map_or(&pipeline.blank, |t| &t.bind_group);
                    scene.set_bind_group(0, group, &[]);
                    scene.draw(draw.range.clone(), 0..1);
                }
            }
            let mut compose = pass(
                encoder,
                attachment(&layers.base_view, None, wgpu::LoadOp::Load),
            );
            viewport(&mut compose);
            compose.set_scissor_rect(rx, ry, rw, rh);
            compose.set_pipeline(&pipeline.compose);
            compose.set_vertex_buffer(0, prepared.composite.slice(..));
            compose.set_bind_group(0, &layers.scratch, &[]);
            let first = index as u32 * 3;
            compose.draw(first..first + 3, 0..1);
        }
        let mut present = pass(encoder, attachment(target, None, wgpu::LoadOp::Load));
        present.set_viewport(ox, oy, bw as f32, bh as f32, 0.0, 1.0);
        let Some((x, y, width, height)) = present_scissor(clip_bounds, (ox, oy), (bw, bh)) else {
            return;
        };
        present.set_scissor_rect(x, y, width, height);
        present.set_pipeline(&pipeline.present);
        present.set_vertex_buffer(0, prepared.composite.slice(..));
        present.set_bind_group(0, &layers.base, &[]);
        let first = prepared.layers.len() as u32 * 3;
        present.draw(first..first + 3, 0..1);
    }
}

/// Shorthand for callers: the marks a resting Cube shows.
pub(super) fn marks(cube: &crate::app::settings::CubeSettings) -> CubeMarks {
    let wallet = super::launcher_presenter::wallet_face_mark(cube);
    CubeMarks {
        network: cube.network,
        vault: wallet == super::launcher_presenter::WalletFaceMark::Vault,
        imported_bitcoin: wallet == super::launcher_presenter::WalletFaceMark::ImportedBitcoin,
        spark: crate::app::features::spark(cube.network).is_available(),
        passkey: cube.is_passkey_cube(),
        backup: super::launcher_presenter::backup_mark(cube),
        ghost: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::ChainId;
    use crate::home::launcher_scene::{BackupMark, DESIGN_STAGE_HEIGHT};

    #[test]
    fn create_placeholder_has_no_wallet_or_connect_art() {
        for light in [false, true] {
            let layout = Layout::new(1440.0, 742.0);
            let stage = StageData::create_placeholder(&layout, light, 0.0);
            assert!(stage.floor);
            assert_eq!(stage.layers.len(), 1);
            assert!(stage.layers[0].dashed);
            assert!(stage.layers[0].footprint.is_none());
            assert!(stage.layers[0].faces.iter().all(|face| face.art.is_none()));
        }
    }

    #[test]
    fn create_placeholder_rises_by_its_hover_lift() {
        let layout = Layout::new(1440.0, 742.0);
        let top = |lift| {
            StageData::create_placeholder(&layout, false, lift).layers[0]
                .faces
                .iter()
                .flat_map(|face| face.projected.corners)
                .map(|(_, y)| y)
                .fold(f32::INFINITY, f32::min)
        };
        let lift = launcher_scene::BOB_HEIGHT;
        assert!((top(0.0) - top(lift) - lift * layout.scale).abs() < 0.5);
    }

    #[test]
    fn spark_is_marked_on_every_cube_whose_network_supports_it() {
        let cube = |chain| {
            crate::app::settings::CubeSettings::new_with_raw_id("c".into(), "C".into(), chain)
        };
        assert!(marks(&cube(ChainId::Bitcoin)).spark);
        assert!(marks(&cube(ChainId::Regtest)).spark);
        assert!(!marks(&cube(ChainId::Testnet4)).spark);
        assert!(!marks(&cube(ChainId::Signet)).spark);
    }

    #[test]
    fn rest_faces_are_rasterized_once_with_a_full_mip_chain() {
        let marks = CubeMarks {
            network: ChainId::Bitcoin,
            vault: true,
            imported_bitcoin: false,
            spark: false,
            passkey: false,
            backup: BackupMark::Confirmed,
            ghost: None,
        };
        let svg = rest_face_svg(Face::Top, marks, false).unwrap();
        let a = art(&svg, 64).expect("coin face rasterizes");
        let b = art(&svg, 64).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(a.levels.len(), 7);
        assert_eq!(a.levels.last().unwrap().0, 1);
        // The supplied coin art is opaque at the face center.
        let (size, pixels) = &a.levels[0];
        let center = ((size / 2) * size + size / 2) as usize * 4;
        assert_eq!(pixels[center + 3], 255);
    }

    #[test]
    fn a_scrolled_stage_presents_at_its_signed_origin_inside_the_clip() {
        let clip = Rectangle {
            x: 0,
            y: 128,
            width: 2880,
            height: 1400,
        };
        // Stage top 300 texels above the window: only its visible part is
        // scissored, while the viewport keeps the negative origin.
        assert_eq!(
            present_scissor(&clip, (0.0, -300.0), (2880, 1484)),
            Some((0, 128, 2880, 1056))
        );
        assert_eq!(
            present_scissor(&clip, (0.0, 200.0), (2880, 1484)),
            Some((0, 200, 2880, 1328))
        );
        assert_eq!(present_scissor(&clip, (0.0, -2000.0), (2880, 1484)), None);
    }

    #[test]
    fn panel_art_never_exceeds_the_byte_budget() {
        for i in 0..40 {
            let svg = details_svg(
                ChainId::Bitcoin,
                i % 2 == 0,
                FaceText {
                    name: &format!("Budget {i}"),
                    lock: "Locked",
                    backup: "Recorded",
                    connect: "Signed in",
                    wallets: "Vault",
                    passkey: false,
                },
            );
            assert!(art(&svg, PANEL_ART_PX).is_some());
            // 40 distinct 840px panels are about 150MB unbounded.
            let retained = art_cache().lock().unwrap().1;
            assert!(retained <= ART_BUDGET, "{} bytes retained", retained);
        }
    }

    #[test]
    fn stage_paints_peers_dimmed_and_settled_faces_without_snapshots() {
        let layout = Layout::new(1440.0, DESIGN_STAGE_HEIGHT);
        let marks = CubeMarks {
            network: ChainId::Bitcoin,
            vault: false,
            imported_bitcoin: false,
            spark: false,
            passkey: false,
            backup: BackupMark::NotConfirmed,
            ghost: None,
        };
        let text = FaceText {
            name: "Everyday",
            lock: "Locked",
            backup: "Not confirmed",
            connect: "Signed out",
            wallets: "Spark",
            passkey: false,
        };
        let cubes = |pose: Pose| {
            vec![
                StageCube {
                    rest: layout.rest_center(0, 2),
                    pose: Pose::DIMMED,
                    marks,
                    details: None,
                    ghost_details: None,
                    unlock_name: None,
                    glow: None,
                },
                StageCube {
                    rest: layout.rest_center(1, 2),
                    pose,
                    marks,
                    details: Some(text),
                    ghost_details: None,
                    unlock_name: Some("Everyday"),
                    glow: None,
                },
            ]
        };
        let settled = StageData::new(&layout, false, true, cubes(Pose::DETAILS));
        let opacities = settled.opacities();
        assert!((opacities[0] - 0.4).abs() < 1e-6 && opacities[1] == 1.0);
        assert_eq!(settled.visible()[1], vec![Face::BackLeft]);
        assert!(!settled.has_art(1, Face::BackLeft), "live panel owns it");
        let turning = StageData::new(
            &layout,
            false,
            true,
            cubes(Pose {
                spin: 0.6,
                ..Pose::DETAILS
            }),
        );
        assert!(turning.has_art(1, Face::BackLeft));
        let unlocking = StageData::new(&layout, true, true, cubes(Pose::unlock(0.5)));
        assert!(unlocking.has_art(1, Face::BackRight));
        let unlocked = StageData::new(&layout, true, true, cubes(Pose::unlock(1.0)));
        assert_eq!(unlocked.visible()[1], vec![Face::BackRight]);
        assert!(!unlocked.has_art(1, Face::BackRight), "live PIN owns it");
    }

    #[test]
    fn a_ghost_paints_every_edge_with_no_shadow_and_a_faint_rest() {
        let layout = Layout::new(1440.0, DESIGN_STAGE_HEIGHT);
        let local = CubeMarks {
            network: ChainId::Bitcoin,
            vault: false,
            imported_bitcoin: false,
            spark: true,
            passkey: false,
            backup: BackupMark::Confirmed,
            ghost: None,
        };
        let ghost = CubeMarks::ghost(ChainId::Bitcoin, launcher_scene::Ghost::Unrestorable);
        let cube = |marks: CubeMarks, index: usize| StageCube {
            rest: layout.rest_center(index, 2),
            pose: Pose::REST,
            marks,
            details: None,
            ghost_details: None,
            unlock_name: None,
            glow: None,
        };
        let data = StageData::new(&layout, false, true, [cube(local, 0), cube(ghost, 1)]);
        assert_eq!(data.visible()[0].len(), 3);
        assert_eq!(data.visible()[1].len(), Face::ALL.len());
        assert_eq!(data.opacities(), vec![1.0, 0.85]);
        assert!(data.layers[0].frame.shadow_opacity > 0.0);
        assert_eq!(data.layers[1].frame.shadow_opacity, 0.0);
        assert!(data.layers[0].footprint.is_none());
        assert_eq!(data.layers[1].footprint, Some(1.0));
        assert!(data.layers[0].faces.iter().all(|f| f.alpha == 1.0));
        assert!(data.layers[1].faces.iter().all(|f| f.alpha == 0.0));
        // Far faces carry no art: only their edges show through.
        assert!(data.layers[1]
            .faces
            .iter()
            .filter(|f| f.projected.area() <= 0.5)
            .all(|f| f.art.is_none()));
        assert_eq!(
            data.layers[1].edge,
            launcher_scene::ghost_edge(ChainId::Bitcoin, false)
        );

        // Selected, it turns a solid details face to the viewer.
        let turned = StageData::new(
            &layout,
            false,
            true,
            [StageCube {
                pose: Pose::DETAILS,
                ghost_details: Some(GhostText {
                    name: "Old laptop",
                    backup: "No Recovery Kit in Connect",
                    restorable: false,
                }),
                ..cube(ghost, 1)
            }],
        );
        let panel = turned.layers[0]
            .faces
            .iter()
            .find(|f| f.projected.face == Face::BackLeft)
            .unwrap();
        assert_eq!(panel.alpha, 1.0);
        assert_eq!(turned.layers[0].footprint, Some(0.0));
    }
}
