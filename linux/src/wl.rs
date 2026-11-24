//! ZTile's own windows, as plain Wayland surfaces (no toolkit):
//! - one invisible 1x1 "proxy" per group: its unique app id (with a matching
//!   .desktop file) gives the group its own taskbar button
//! - a "+" placeholder in each empty cell
//! - the drop overlay shown while dragging
//!
//! KWin's script positions them. Their content is a single-pixel buffer stretched
//! by the viewporter (no memory however large the window), plus a small "+" image.

use std::collections::HashMap;
use std::os::fd::AsFd;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_pointer, wl_pointer::WlPointer, wl_region::WlRegion,
    wl_registry::WlRegistry, wl_seat, wl_seat::WlSeat, wl_shm, wl_shm::WlShm, wl_shm_pool::WlShmPool,
    wl_subcompositor::WlSubcompositor, wl_subsurface::WlSubsurface, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1 as Spbm;
use wayland_protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_surface::XdgSurface, xdg_toplevel, xdg_toplevel::XdgToplevel, xdg_wm_base, xdg_wm_base::XdgWmBase};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Proxy(u32),
    Plus(u32, usize),
    Overlay,
}

pub enum WlEvent {
    /// "Close" on a window (e.g. from its taskbar button's menu)
    Closed(Role),
    /// left click on a window
    Clicked(Role),
}

struct Glyph {
    surface: WlSurface,
    sub: WlSubsurface,
    vp: WpViewport,
}

struct Win {
    id: u32,
    role: Role,
    surface: WlSurface,
    xdg: XdgSurface,
    top: XdgToplevel,
    vp: WpViewport,
    color: [u8; 4],
    size: (i32, i32),
    pending: (i32, i32),
    configured: bool,
    glyph: Option<Glyph>,
}

pub struct State {
    compositor: WlCompositor,
    subcompositor: Option<WlSubcompositor>,
    wm: XdgWmBase,
    viewporter: WpViewporter,
    spbm: Option<Spbm>,
    shm: WlShm,
    wins: Vec<Win>,
    next_id: u32,
    pointer_on: Option<WlSurface>,
    solid: HashMap<[u8; 4], WlBuffer>,
    glyph_buf: Option<WlBuffer>,
    events: Vec<WlEvent>,
}

pub struct Wl {
    pub queue: EventQueue<State>,
    qh: QueueHandle<State>,
    pub state: State,
}

pub const GLYPH: i32 = 48; // logical px of the "+"
const PLUS_BG: [u8; 4] = [0x20, 0x20, 0x20, 255];

impl Wl {
    pub fn connect() -> Result<Wl, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland session ({e})"))?;
        let (globals, queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let need = |what: &str| format!("the compositor lacks {what}");
        let compositor: WlCompositor = globals.bind(&qh, 4..=6, ()).map_err(|_| need("wl_compositor"))?;
        let wm: XdgWmBase = globals.bind(&qh, 1..=6, ()).map_err(|_| need("xdg_wm_base"))?;
        let viewporter: WpViewporter = globals.bind(&qh, 1..=1, ()).map_err(|_| need("wp_viewporter"))?;
        let shm: WlShm = globals.bind(&qh, 1..=1, ()).map_err(|_| need("wl_shm"))?;
        let subcompositor = globals.bind(&qh, 1..=1, ()).ok();
        let spbm = globals.bind(&qh, 1..=1, ()).ok();
        if let Ok(seat) = globals.bind::<WlSeat, _, _>(&qh, 1..=7, ()) {
            let _ = seat; // pointer is created when the seat reports its capabilities
        }
        let state = State {
            compositor,
            subcompositor,
            wm,
            viewporter,
            spbm,
            shm,
            wins: Vec::new(),
            next_id: 1,
            pointer_on: None,
            solid: HashMap::new(),
            glyph_buf: None,
            events: Vec::new(),
        };
        Ok(Wl { queue, qh, state })
    }

    /// Create a window. app_id None = ZTile's own id.
    pub fn create(&mut self, role: Role, app_id: &str, title: &str) -> u32 {
        let s = &mut self.state;
        let qh = &self.qh;
        let id = s.next_id;
        s.next_id += 1;
        let surface = s.compositor.create_surface(qh, ());
        let xdg = s.wm.get_xdg_surface(&surface, qh, id);
        let top = xdg.get_toplevel(qh, id);
        top.set_app_id(app_id.to_string());
        top.set_title(title.to_string());
        let vp = s.viewporter.get_viewport(&surface, qh, ());
        let color = match role {
            Role::Proxy(_) => [0, 0, 0, 0],
            Role::Plus(..) => PLUS_BG,
            Role::Overlay => [0, 0, 0, 0],
        };
        if role == Role::Overlay {
            // clicks go through to the windows below
            let empty: WlRegion = s.compositor.create_region(qh, ());
            surface.set_input_region(Some(&empty));
            empty.destroy();
        }
        let glyph = match (role, &s.subcompositor) {
            (Role::Plus(..), Some(sc)) => {
                let gs = s.compositor.create_surface(qh, ());
                let sub = sc.get_subsurface(&gs, &surface, qh, ());
                let gvp = s.viewporter.get_viewport(&gs, qh, ());
                let empty: WlRegion = s.compositor.create_region(qh, ());
                gs.set_input_region(Some(&empty)); // clicks land on the cell itself
                empty.destroy();
                Some(Glyph { surface: gs, sub, vp: gvp })
            }
            _ => None,
        };
        surface.commit(); // initial commit without a buffer: asks for a configure
        s.wins.push(Win { id, role, surface, xdg, top, vp, color, size: (1, 1), pending: (0, 0), configured: false, glyph });
        id
    }

    pub fn destroy(&mut self, id: u32) {
        let s = &mut self.state;
        if let Some(i) = s.wins.iter().position(|w| w.id == id) {
            let w = s.wins.remove(i);
            if let Some(g) = w.glyph {
                g.vp.destroy();
                g.sub.destroy();
                g.surface.destroy();
            }
            w.vp.destroy();
            w.top.destroy();
            w.xdg.destroy();
            w.surface.destroy();
            if s.pointer_on.as_ref() == Some(&w.surface) {
                s.pointer_on = None;
            }
        }
    }

    pub fn set_title(&mut self, id: u32, title: &str) {
        if let Some(w) = self.state.wins.iter().find(|w| w.id == id) {
            w.top.set_title(title.to_string());
        }
    }

    pub fn set_color(&mut self, id: u32, rgba: [u8; 4]) {
        let qh = self.qh.clone();
        let s = &mut self.state;
        if let Some(i) = s.wins.iter().position(|w| w.id == id) {
            s.wins[i].color = rgba;
            if s.wins[i].configured {
                s.draw(i, &qh);
            }
        }
    }

    pub fn take_events(&mut self) -> Vec<WlEvent> {
        std::mem::take(&mut self.state.events)
    }
}

impl State {
    /// A buffer of one premultiplied colour, shared by every surface that uses it.
    fn solid(&mut self, rgba: [u8; 4], qh: &QueueHandle<State>) -> WlBuffer {
        if let Some(b) = self.solid.get(&rgba) {
            return b.clone();
        }
        let a = rgba[3] as u64;
        let pm = |c: u8| (c as u64 * a / 255) as u8;
        let b = match &self.spbm {
            Some(m) => {
                let wide = |c: u8| (c as u64 * u32::MAX as u64 / 255) as u32;
                m.create_u32_rgba_buffer(wide(pm(rgba[0])), wide(pm(rgba[1])), wide(pm(rgba[2])), wide(rgba[3]), qh, ())
            }
            None => {
                let px = u32::from_be_bytes([rgba[3], pm(rgba[0]), pm(rgba[1]), pm(rgba[2])]).to_ne_bytes();
                shm_buffer(&self.shm, &px, 1, 1, qh).expect("shm buffer")
            }
        };
        self.solid.insert(rgba, b.clone());
        b
    }

    fn glyph_buffer(&mut self, qh: &QueueHandle<State>) -> Option<WlBuffer> {
        if self.glyph_buf.is_none() {
            let size = (GLYPH * 2) as u32; // drawn at 2x: sharp with display scaling
            let c = crate::icon::plus_glyph(size);
            self.glyph_buf = shm_buffer(&self.shm, &c.to_argb_premul(), size as i32, size as i32, qh);
        }
        self.glyph_buf.clone()
    }

    fn draw(&mut self, i: usize, qh: &QueueHandle<State>) {
        let buf = self.solid(self.wins[i].color, qh);
        let glyph_buf = if self.wins[i].glyph.is_some() { self.glyph_buffer(qh) } else { None };
        let w = &mut self.wins[i];
        let (cw, ch) = (w.size.0.max(1), w.size.1.max(1));
        if let (Some(g), Some(gb)) = (&w.glyph, glyph_buf) {
            let side = GLYPH.min(cw).min(ch);
            g.vp.set_destination(side, side);
            g.sub.set_position((cw - side) / 2, (ch - side) / 2);
            g.surface.attach(Some(&gb), 0, 0);
            g.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
            g.surface.commit();
        }
        w.vp.set_destination(cw, ch);
        w.surface.attach(Some(&buf), 0, 0);
        w.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        w.surface.commit();
    }
}

/// An shm buffer holding `px` (ARGB8888, native endian). The backing file lives in
/// $XDG_RUNTIME_DIR and is unlinked right away.
fn shm_buffer(shm: &WlShm, px: &[u8], w: i32, h: i32, qh: &QueueHandle<State>) -> Option<WlBuffer> {
    use std::io::Write;
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from)?;
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let path = dir.join(format!(
        ".ztile-shm-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut f = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    f.write_all(px).ok()?;
    let pool: WlShmPool = shm.create_pool(f.as_fd(), px.len() as i32, qh, ());
    let b = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    Some(b)
}

// ---------------------------------------------------------------- events

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as wayland_client::Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<XdgWmBase, ()> for State {
    fn event(_: &mut Self, wm: &XdgWmBase, ev: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = ev {
            wm.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, u32> for State {
    fn event(s: &mut Self, xdg: &XdgSurface, ev: xdg_surface::Event, id: &u32, _: &Connection, qh: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = ev {
            xdg.ack_configure(serial);
            if let Some(i) = s.wins.iter().position(|w| w.id == *id) {
                let w = &mut s.wins[i];
                if w.pending.0 > 0 && w.pending.1 > 0 {
                    w.size = w.pending;
                } else if !w.configured {
                    w.size = match w.role {
                        Role::Plus(..) => (200, 150),
                        _ => (1, 1),
                    };
                }
                w.configured = true;
                s.draw(i, qh);
            }
        }
    }
}

impl Dispatch<XdgToplevel, u32> for State {
    fn event(s: &mut Self, _: &XdgToplevel, ev: xdg_toplevel::Event, id: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let Some(w) = s.wins.iter_mut().find(|w| w.id == *id) else { return };
        match ev {
            xdg_toplevel::Event::Configure { width, height, .. } => w.pending = (width, height),
            xdg_toplevel::Event::Close => s.events.push(WlEvent::Closed(w.role)),
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(_: &mut Self, seat: &WlSeat, ev: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = ev {
            if caps.contains(wl_seat::Capability::Pointer) {
                seat.get_pointer(qh, ());
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(s: &mut Self, _: &WlPointer, ev: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            wl_pointer::Event::Enter { surface, .. } => s.pointer_on = Some(surface),
            wl_pointer::Event::Leave { .. } => s.pointer_on = None,
            wl_pointer::Event::Button { button: 0x110, state: WEnum::Value(wl_pointer::ButtonState::Released), .. } => {
                // BTN_LEFT released over one of our windows
                if let Some(on) = &s.pointer_on {
                    if let Some(w) = s.wins.iter().find(|w| &w.surface == on || w.glyph.as_ref().is_some_and(|g| &g.surface == on)) {
                        s.events.push(WlEvent::Clicked(w.role));
                    }
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlSubcompositor);
delegate_noop!(State: ignore WlSubsurface);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlRegion);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore WpViewporter);
delegate_noop!(State: ignore WpViewport);
delegate_noop!(State: ignore Spbm);
