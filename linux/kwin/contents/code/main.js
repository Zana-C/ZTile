// ZTile for KDE Plasma: the window engine.
//
// On Wayland only the compositor may move other apps' windows, so everything
// that touches windows lives here, inside KWin. The ZTile app (Python/Qt) owns
// the UI, the saved layouts and the per-group taskbar buttons, and talks to this
// script over D-Bus:
//   - script -> app: callDBus(..., "event", json)
//   - app -> script: a long poll, callDBus(..., "poll") answered when there are
//     commands (the app answers "[]" every ~20 s so the call never times out)
//
// Per group:
//   - members lose their own taskbar entries (skipTaskbar)
//   - the app provides one "proxy" window per group (its own app id and .desktop
//     file, so it gets its own taskbar button with a composite icon). The proxy is
//     1x1 and transparent; clicking its button toggles the group. While the group
//     is up the proxy holds the keyboard focus, so members are never activated
//     (chat apps would mark messages as read)
//   - groups stack like windows: showing one raises it, minimizing reveals the
//     one below
//   - a cell remembers its app after the app closes, so it can be started again
//
// Dragging any window shows a drop overlay (a window of the app):
//   - onto another cell: swap (or fill if empty), also across groups
//   - outside every group: leave the group
//   - small move / own cell: snap back
"use strict";

const SERVICE = "io.github.ZanaC.ZTile";
const OBJECT = "/ZTile";
const IFACE = "io.github.ZanaC.ZTile";
const PROXY_PREFIX = "ztile-group-";
const SWAP_MIN_MOVE = 48; // logical px; shorter drags snap back
const LAUNCH_WAIT = 30000; // ms we wait for a started app's window
const SETTLE_TIME = 3000; // ms after adoption in which we re-place apps that move themselves
const SETTLE_TRIES = 3;

let appPid = 0;
let groups = [];
let pending = []; // launches: {gid, cell, spec, before: Set(ids), deadline}
let drag = null; // {w, from: {g, i} | null, sx, sy, resizing, target}
let overlay = null; // the app's drop overlay window
let orphans = []; // app windows that arrived before their group / config
let origSkip = new Map(); // window id -> skipTaskbar before we took it
let guard = 0; // > 0 while we change windows ourselves
let lastToggle = { gid: 0, t: 0 };
let stateTimer = null;
let userChanged = new Set(); // group ids changed by the user since the last state

// ------------------------------------------------------------------ helpers

function log(...a) {
    print("ztile:", ...a);
}

function wid(w) {
    return String(w.internalId);
}

function appIdOf(w) {
    return w.desktopFileName || w.resourceClass || "";
}

function rect(r) {
    return { x: r.x, y: r.y, width: r.width, height: r.height };
}

function contains(r, p) {
    return p.x >= r.x && p.x < r.x + r.width && p.y >= r.y && p.y < r.y + r.height;
}

function intersects(a, b) {
    return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
}

function onCurrentDesktop(w) {
    return w.onAllDesktops || w.desktops.indexOf(workspace.currentDesktop) >= 0;
}

function quiet(f) {
    guard++;
    try {
        f();
    } finally {
        guard--;
    }
}

function isOurs(w) {
    return appPid !== 0 && w.pid === appPid;
}

function isProxy(w) {
    return isOurs(w) && (w.desktopFileName || "").startsWith(PROXY_PREFIX);
}

/** Our "+" placeholder for an empty cell: caption "ZTile cell <group> <cell>". */
function plusInfo(w) {
    if (!isOurs(w)) return null;
    const m = /^ZTile cell (\d+) (\d+)$/.exec(w.caption || "");
    return m ? { gid: Number(m[1]), cell: Number(m[2]) } : null;
}

function isOverlay(w) {
    return isOurs(w) && w.caption === "ZTile overlay";
}

/** A real app window we may tile. */
function isCandidate(w) {
    return (
        w.normalWindow &&
        w.managed &&
        !w.deleted &&
        !w.specialWindow &&
        !w.transient &&
        !w.fullScreen &&
        !isOurs(w) &&
        w.moveable &&
        w.resizeable
    );
}

function groupById(id) {
    return groups.find((g) => g.id === id) || null;
}

/** {g, i} of a member window */
function locate(w) {
    for (const g of groups) {
        const i = g.slots.findIndex((s) => s.w === w);
        if (i >= 0) return { g, i };
    }
    return null;
}

function members(g) {
    return g.slots.filter((s) => s.w).map((s) => s.w);
}

// ------------------------------------------------------------------ D-Bus

function send(ev, data) {
    const msg = Object.assign({ ev: ev }, data || {});
    callDBus(SERVICE, OBJECT, IFACE, "event", JSON.stringify(msg));
}

function poll() {
    callDBus(SERVICE, OBJECT, IFACE, "poll", function (reply) {
        let cmds = [];
        try {
            cmds = JSON.parse(reply);
        } catch (e) {
            log("bad poll reply", e);
        }
        for (const c of Array.isArray(cmds) ? cmds : []) {
            try {
                handle(c);
            } catch (e) {
                log("command failed", c && c.cmd, e);
            }
        }
        poll();
    });
    // If the app is gone the call fails and the loop stops; the app reloads this
    // script when it starts again. Groups dissolve when their proxy windows close.
}

// ------------------------------------------------------------------ input checks
// Commands come from the ZTile app over the session bus. They are checked anyway:
// a malformed command must not be able to break KWin's state (NaN geometry...).

function isInt(v, min, max) {
    return typeof v === "number" && Number.isInteger(v) && v >= min && v <= max;
}

function isStr(v, max) {
    return typeof v === "string" && v.length <= max;
}

/** {app, window} with sane strings, or null */
function cleanSpec(s) {
    if (!s || typeof s !== "object" || !isStr(s.app, 256) || !s.app) return null;
    return { app: s.app, window: isStr(s.window, 256) ? s.window : "" };
}

/** A group from an "open" command, or null if anything is off. */
function cleanGroup(d) {
    if (!d || typeof d !== "object" || !isInt(d.id, 1, 1e9)) return null;
    if (!Array.isArray(d.cells) || d.cells.length < 1 || d.cells.length > 16) return null;
    const cells = [];
    for (const c of d.cells) {
        if (!Array.isArray(c) || c.length !== 4) return null;
        if (!c.every((v) => typeof v === "number" && isFinite(v) && v >= 0 && v <= 1)) return null;
        if (!(c[0] < c[2] && c[1] < c[3])) return null;
        cells.push([c[0], c[1], c[2], c[3]]);
    }
    const specs = Array.isArray(d.specs) ? d.specs : [];
    if (!isStr(d.proxyApp, 140) || !/^ztile-group-[a-z0-9_]{1,120}$/.test(d.proxyApp)) return null;
    const m = d.monitor && typeof d.monitor === "object" ? d.monitor : {};
    return {
        id: d.id,
        name: isStr(d.name, 200) ? d.name : "",
        proxyApp: d.proxyApp,
        monitor: {
            name: isStr(m.name, 64) ? m.name : "",
            width: isInt(m.width, 0, 100000) ? m.width : 0,
            height: isInt(m.height, 0, 100000) ? m.height : 0,
        },
        cells: cells,
        gap: isInt(d.gap, 0, 64) ? d.gap : 0,
        borderless: d.borderless === true,
        specs: cells.map((_, i) => cleanSpec(specs[i])),
    };
}

function handle(c) {
    if (!c || typeof c !== "object") return;
    switch (c.cmd) {
        case "config":
            appPid = isInt(c.pid, 1, 1e9) ? c.pid : 0;
            for (const w of workspace.windowList()) classify(w);
            sendOutputs();
            break;
        case "open": {
            const g = cleanGroup(c.group);
            if (g) openGroup(g, c.launch === true);
            break;
        }
        case "close":
            closeGroup(groupById(c.id), true);
            break;
        case "closeAll":
            for (const g of groups.slice()) closeGroup(g, true);
            break;
        case "show": {
            const g = groupById(c.id);
            if (g) showGroup(g, true);
            break;
        }
        case "fill":
            fillFromApp(c);
            break;
        case "launch": {
            const g = groupById(c.id);
            const spec = cleanSpec(c.spec);
            if (g && isInt(c.cell, 0, g.slots.length - 1) && spec) {
                g.slots[c.cell].spec = spec;
                if (c.user === true) userChanged.add(g.id);
                requestLaunch(g, c.cell, spec);
                stateChanged();
            }
            break;
        }
        case "windows":
            if (isInt(c.req, 1, 1e12)) send("windows", { req: c.req, list: listWindows() });
            break;
        case "overlayState":
            if (overlay) idleOverlay();
            break;
    }
}

// ------------------------------------------------------------------ geometry

function outputs() {
    return workspace.screens;
}

function physSize(o) {
    const s = o.devicePixelRatio || 1;
    return { w: Math.round(o.geometry.width * s), h: Math.round(o.geometry.height * s) };
}

function sendOutputs() {
    send("outputs", {
        list: outputs().map((o) => {
            const p = physSize(o);
            return {
                name: o.name,
                width: p.w,
                height: p.h,
                label: [o.manufacturer, o.model].filter((x) => x).join(" "),
                x: o.geometry.x,
                y: o.geometry.y,
                active: o === workspace.activeScreen,
            };
        }),
    });
}

/** Same name and size > same size > same name > the active output. */
function resolveOutput(m) {
    const outs = outputs();
    m = m || {};
    const same = (o) => {
        const p = physSize(o);
        return p.w === m.width && p.h === m.height;
    };
    return (
        outs.find((o) => o.name === m.name && same(o)) ||
        outs.find(same) ||
        outs.find((o) => o.name === m.name) ||
        workspace.activeScreen ||
        outs[0]
    );
}

function layoutGroup(g) {
    g.output = resolveOutput(g.monitor);
    const a = workspace.clientArea(KWin.MaximizeArea, g.output, workspace.currentDesktop);
    g.area = rect(a);
    const s = g.output.devicePixelRatio || 1;
    const snap = (v) => Math.round(v * s) / s; // whole device pixels
    const gap = g.gap;
    const edge = (atBorder) => (atBorder ? gap : gap / 2);
    g.cells = g.ncells.map((c) => {
        const l = snap(a.x + c[0] * a.width + edge(c[0] <= 0));
        const t = snap(a.y + c[1] * a.height + edge(c[1] <= 0));
        const r = snap(a.x + c[2] * a.width - edge(c[2] >= 1));
        const b = snap(a.y + c[3] * a.height - edge(c[3] >= 1));
        return { x: l, y: t, width: Math.max(1, r - l), height: Math.max(1, b - t) };
    });
}

function toCurrentDesktop(w) {
    if (!onCurrentDesktop(w)) w.desktops = [workspace.currentDesktop];
}

function place(w, r) {
    if (!w || w.deleted) return;
    quiet(() => {
        if (w.fullScreen) w.fullScreen = false;
        if (w.maximizeMode !== 0) w.setMaximize(false, false);
        w.frameGeometry = { x: r.x, y: r.y, width: r.width, height: r.height };
    });
}

function placeSlot(g, i) {
    const s = g.slots[i];
    if (s.w) place(s.w, g.cells[i]);
}

function placePlus(g, i) {
    const p = g.plus[i];
    if (!p) return;
    quiet(() => {
        p.frameGeometry = rect(g.cells[i]);
    });
}

// ------------------------------------------------------------------ groups

function newGroup(d) {
    const n = d.cells.length;
    const g = {
        id: d.id,
        name: d.name || "",
        proxyApp: d.proxyApp || PROXY_PREFIX + d.id,
        monitor: d.monitor || {},
        ncells: d.cells,
        gap: d.gap,
        borderless: d.borderless,
        slots: [],
        plus: [],
        proxy: null,
        minimized: false,
    };
    for (let i = 0; i < n; i++) {
        g.slots.push({ w: null, spec: (d.specs && d.specs[i]) || null, armedUntil: 0, noBorder: null, settle: null });
        g.plus.push(null);
    }
    layoutGroup(g);
    return g;
}

/** How well a window matches a spec. -1 = can't be it. */
function score(spec, w) {
    if (!spec || !spec.app) return -1;
    const want = spec.app.toLowerCase();
    const ids = [w.desktopFileName, w.resourceClass].map((x) => (x || "").toLowerCase());
    if (ids.indexOf(want) < 0) return -1;
    let s = 1;
    if (spec.window) {
        if ((w.caption || "").toLowerCase().indexOf(spec.window.toLowerCase()) < 0) return -1;
        s += 50;
    }
    if (ids[0] === want) s += 10;
    if (!locate(w)) s += 5; // prefer windows that aren't in another group
    return s;
}

/** Best open window per spec; each window used once, strongest matches first. */
function matchAll(specs) {
    const wins = workspace.windowList().filter(isCandidate);
    const pairs = [];
    specs.forEach((spec, si) => {
        if (!spec) return;
        wins.forEach((w, wi) => {
            const s = score(spec, w);
            if (s >= 0) pairs.push([s, si, wi]);
        });
    });
    pairs.sort((a, b) => b[0] - a[0]);
    const out = specs.map(() => null);
    const used = new Set();
    for (const [, si, wi] of pairs) {
        if (!out[si] && !used.has(wi)) {
            out[si] = wins[wi];
            used.add(wi);
        }
    }
    return out;
}

function openGroup(d, launch) {
    if (groupById(d.id)) closeGroup(groupById(d.id), true);
    const g = newGroup(d);
    groups.push(g);
    const wins = matchAll(g.slots.map((s) => s.spec));
    wins.forEach((w, i) => {
        if (w) adopt(g, i, w);
    });
    for (const w of orphans.slice()) classify(w);
    showGroup(g, true);
    if (launch) {
        g.slots.forEach((s, i) => {
            if (!s.w && s.spec) requestLaunch(g, i, s.spec);
        });
    }
    stateChanged();
}

/** Take a window into cell i. */
function adopt(g, i, w) {
    const old = locate(w);
    if (old && (old.g !== g || old.i !== i)) {
        release(old.g, old.i, false, false);
        userChanged.add(old.g.id);
    }
    const s = g.slots[i];
    if (s.w && s.w !== w) release(g, i, true, true);
    s.w = w;
    const spec = { app: appIdOf(w), window: "" };
    if (s.spec && s.spec.app && s.spec.app.toLowerCase() === spec.app.toLowerCase()) {
        spec.window = s.spec.window || ""; // keep the user's pinned window
    }
    s.spec = spec;
    const id = wid(w);
    if (!origSkip.has(id)) origSkip.set(id, w.skipTaskbar);
    quiet(() => {
        w.skipTaskbar = true;
        if (g.borderless && !w.noBorder) {
            s.noBorder = false;
            w.noBorder = true;
        }
        if (g.minimized) {
            w.minimized = true;
        } else {
            if (w.minimized) w.minimized = false;
            toCurrentDesktop(w);
        }
    });
    if (!g.minimized) placeSlot(g, i);
    s.settle = { until: Date.now() + SETTLE_TIME, tries: 0 };
}

/**
 * Free cell i.
 * giveBack: the window stays a normal window (taskbar entry, title bar back)
 * keepSpec: the cell still belongs to that app (it only closed)
 */
function release(g, i, giveBack, keepSpec) {
    const s = g.slots[i];
    if (!keepSpec) s.spec = null;
    s.armedUntil = 0;
    s.settle = null;
    const w = s.w;
    if (!w) return;
    s.w = null;
    if (!w.deleted) {
        quiet(() => {
            if (s.noBorder !== null) w.noBorder = s.noBorder;
            if (giveBack) {
                const id = wid(w);
                w.skipTaskbar = origSkip.has(id) ? origSkip.get(id) : false;
                origSkip.delete(id);
            }
        });
    }
    s.noBorder = null;
}

function closeGroup(g, giveBack) {
    if (!g) return;
    groups = groups.filter((x) => x !== g);
    for (let i = 0; i < g.slots.length; i++) {
        const w = g.slots[i].w;
        if (w && !w.deleted && w.minimized) quiet(() => (w.minimized = false));
        release(g, i, giveBack, false);
    }
    pending = pending.filter((p) => p.gid !== g.id);
    stateChanged();
}

/** Ground truth from the stacking order: is our group what you see on its area? */
function onTop(g) {
    const ms = members(g);
    if (g.minimized || ms.some((w) => w.minimized || !onCurrentDesktop(w))) return false;
    const stack = workspace.stackingOrder;
    for (let k = stack.length - 1; k >= 0; k--) {
        const w = stack[k];
        if (!w.normalWindow || w.minimized || w.keepAbove || w.deleted || !onCurrentDesktop(w)) continue;
        if (isOverlay(w) || isProxy(w)) continue;
        if (!intersects(w.frameGeometry, g.area)) continue;
        if (ms.indexOf(w) >= 0) return true;
        const p = plusInfo(w);
        return !!p && p.gid === g.id;
    }
    return true;
}

/** Bring a group up without activating any member. */
function showGroup(g, parkFocus) {
    g.minimized = false;
    layoutGroup(g);
    quiet(() => {
        g.slots.forEach((s, i) => {
            if (!s.w) return;
            if (s.w.minimized) s.w.minimized = false;
            toCurrentDesktop(s.w);
        });
        g.plus.forEach((p) => {
            if (!p) return;
            if (p.minimized) p.minimized = false;
            toCurrentDesktop(p);
        });
    });
    g.slots.forEach((s, i) => placeSlot(g, i));
    g.plus.forEach((p, i) => placePlus(g, i));
    quiet(() => {
        for (const w of members(g)) workspace.raiseWindow(w);
        for (const p of g.plus) if (p) workspace.raiseWindow(p);
        if (g.proxy) {
            if (g.proxy.minimized) g.proxy.minimized = false;
            // the invisible proxy keeps the focus, so no member gets activated
            if (parkFocus) workspace.activeWindow = g.proxy;
        }
    });
    stateChanged();
}

function minimizeGroup(g) {
    g.minimized = true;
    quiet(() => {
        for (const w of members(g)) w.minimized = true;
        for (const p of g.plus) if (p) p.minimized = true;
        if (g.proxy && !g.proxy.minimized) g.proxy.minimized = true;
    });
    stateChanged();
}

/** Taskbar button clicked: group on top -> minimize it, otherwise bring it up. */
function proxyClicked(g) {
    const now = Date.now();
    if (lastToggle.gid === g.id && now - lastToggle.t < 400) return;
    lastToggle = { gid: g.id, t: now };
    if (onTop(g)) minimizeGroup(g);
    else showGroup(g, true);
}

// ------------------------------------------------------------------ launching

function requestLaunch(g, i, spec) {
    const before = new Set(workspace.windowList().map(wid));
    pending = pending.filter((p) => !(p.gid === g.id && p.cell === i));
    pending.push({ gid: g.id, cell: i, spec: spec, before: before, deadline: Date.now() + LAUNCH_WAIT });
    send("launch", { id: g.id, cell: i, spec: spec });
}

/** A new window appeared: is it one we're waiting for? */
function catchLaunched(w) {
    const now = Date.now();
    pending = pending.filter((p) => p.deadline > now && groupById(p.gid));
    if (!pending.length || !isCandidate(w) || locate(w)) return;
    let best = null;
    for (const p of pending) {
        if (p.before.has(wid(w))) continue;
        const s = score(p.spec, w);
        if (s >= 0 && (!best || s > best.s)) best = { s: s, p: p };
    }
    if (!best) return;
    const p = best.p;
    pending = pending.filter((x) => x !== p);
    const g = groupById(p.gid);
    if (!g || g.slots[p.cell].w) return;
    adopt(g, p.cell, w);
    g.slots[p.cell].armedUntil = p.deadline; // a splash screen may close: keep waiting
    if (!g.minimized) quiet(() => workspace.raiseWindow(w));
    stateChanged();
}

// ------------------------------------------------------------------ app windows

function classify(w) {
    if (!isOurs(w)) return;
    orphans = orphans.filter((x) => x !== w);
    if (isOverlay(w)) {
        overlay = w;
        idleOverlay();
        return;
    }
    if (isProxy(w)) {
        const g = groups.find((x) => x.proxyApp === w.desktopFileName);
        if (!g) {
            orphans.push(w);
            return;
        }
        g.proxy = w;
        quiet(() => {
            w.skipSwitcher = true;
            w.skipPager = true;
            w.noBorder = true;
            w.keepBelow = true;
            w.onAllDesktops = true; // its button is there on every desktop
            w.frameGeometry = { x: g.area.x, y: g.area.y, width: 1, height: 1 };
            if (g.minimized) w.minimized = true;
            else workspace.activeWindow = w;
        });
        return;
    }
    const p = plusInfo(w);
    if (p) {
        const g = groupById(p.gid);
        if (!g || p.cell >= g.plus.length) {
            orphans.push(w);
            return;
        }
        g.plus[p.cell] = w;
        quiet(() => {
            w.skipTaskbar = true;
            w.skipSwitcher = true;
            w.skipPager = true;
            w.noBorder = true;
            if (g.minimized) w.minimized = true;
            else {
                toCurrentDesktop(w);
                workspace.raiseWindow(w);
            }
        });
        placePlus(g, p.cell);
    }
}

function idleOverlay() {
    if (!overlay || overlay.deleted) return;
    quiet(() => {
        overlay.skipTaskbar = true;
        overlay.skipSwitcher = true;
        overlay.skipPager = true;
        overlay.noBorder = true;
        overlay.keepAbove = false;
        overlay.keepBelow = true;
        overlay.onAllDesktops = true;
        overlay.opacity = 0;
        overlay.frameGeometry = { x: workspace.activeScreen.geometry.x, y: workspace.activeScreen.geometry.y, width: 1, height: 1 };
    });
}

function showOverlay(r) {
    if (!overlay || overlay.deleted) return;
    quiet(() => {
        overlay.keepBelow = false;
        overlay.keepAbove = true;
        overlay.frameGeometry = rect(r);
        overlay.opacity = 1;
        workspace.raiseWindow(overlay);
    });
}

function fillFromApp(c) {
    const g = groupById(c.id);
    if (!g || !isInt(c.cell, 0, g.slots.length - 1) || !isStr(c.win, 64) || g.slots[c.cell].w) return;
    const w = workspace.windowList().find((x) => wid(x) === c.win);
    if (!w || !isCandidate(w)) return;
    adopt(g, c.cell, w);
    if (!g.minimized) quiet(() => workspace.raiseWindow(w));
    userChanged.add(g.id);
    stateChanged();
}

function listWindows() {
    return workspace
        .windowList()
        .filter(isCandidate)
        .map((w) => {
            const loc = locate(w);
            return { id: wid(w), caption: w.caption, app: appIdOf(w), group: loc ? loc.g.id : null };
        });
}

// ------------------------------------------------------------------ state for the app

function stateChanged() {
    if (!stateTimer) {
        stateTimer = new QTimer();
        stateTimer.singleShot = true;
        stateTimer.interval = 30;
        stateTimer.timeout.connect(sendState);
    }
    if (!stateTimer.active) stateTimer.start();
}

function sendState() {
    const list = groups.map((g) => ({
        id: g.id,
        minimized: g.minimized,
        user: userChanged.has(g.id),
        cells: g.slots.map((s) => ({
            // no window titles here: they can be private and the app doesn't need them
            win: s.w ? { id: wid(s.w), app: appIdOf(s.w) } : null,
            spec: s.spec,
        })),
    }));
    userChanged.clear();
    send("state", { groups: list });
}

// ------------------------------------------------------------------ dragging

/** Group cell under the cursor, by what is actually visible there. */
function cellAt(pt, dragged) {
    const stack = workspace.stackingOrder;
    for (let k = stack.length - 1; k >= 0; k--) {
        const w = stack[k];
        if (w === dragged || w.deleted || w.minimized || !w.normalWindow || !onCurrentDesktop(w)) continue;
        if (isOverlay(w) || isProxy(w)) continue;
        if (!contains(w.frameGeometry, pt)) continue;
        const loc = locate(w);
        const p = plusInfo(w);
        const g = loc ? loc.g : p ? groupById(p.gid) : null;
        if (!g || g.minimized) return null; // some other window covers this spot
        const j = g.cells.findIndex((c) => contains(c, pt));
        return j >= 0 ? { g: g, j: j } : null;
    }
    // nothing there (e.g. a gap between cells): the highest visible group wins
    let best = null;
    for (const g of groups) {
        if (g.minimized) continue;
        const j = g.cells.findIndex((c) => contains(c, pt));
        if (j < 0) continue;
        const z = Math.max(-1, ...members(g).concat(g.plus.filter((p) => p)).map((w) => stack.indexOf(w)));
        if (!best || z > best.z) best = { g: g, j: j, z: z };
    }
    return best;
}

/** The cursor position (a function so the drag logic can be exercised in tests). */
function cursorPoint() {
    return workspace.cursorPos;
}

function dropTarget(d, pt) {
    const f = d.w.frameGeometry;
    const moved = Math.abs(f.x - d.sx) + Math.abs(f.y - d.sy);
    const hit = cellAt(pt, d.w);
    if (d.from) {
        const fg = d.from.g;
        if (hit) {
            if ((hit.g === fg && hit.j === d.from.i) || moved < SWAP_MIN_MOVE) return { kind: "stay", g: fg, j: d.from.i };
            return { kind: hit.g.slots[hit.j].w ? "swap" : "fill", g: hit.g, j: hit.j };
        }
        if (contains(fg.area, pt) || moved < SWAP_MIN_MOVE) return { kind: "stay", g: fg, j: d.from.i };
        return { kind: "leave", g: fg, j: d.from.i };
    }
    if (hit && !hit.g.slots[hit.j].w) return { kind: "fill", g: hit.g, j: hit.j };
    return null;
}

function sameTarget(a, b) {
    return (!a && !b) || (a && b && a.kind === b.kind && a.g === b.g && a.j === b.j);
}

function hideOverlay() {
    send("overlay", { kind: null });
    idleOverlay();
}

function dragStarted(w) {
    const loc = locate(w);
    if (!loc && !isCandidate(w)) return;
    const f = w.frameGeometry;
    // Move or resize is decided by the first step (KWin's flags are reliable by
    // then), once: a window that changes size while crossing to a display with
    // another scale is still being moved.
    drag = { w: w, from: loc, sx: f.x, sy: f.y, resizing: null, target: null };
}

function dragStepped(w) {
    if (!drag || drag.w !== w) return;
    if (drag.resizing === null) drag.resizing = !!w.resize && !w.move;
    if (drag.resizing) return;
    const t = dropTarget(drag, cursorPoint());
    if (sameTarget(t, drag.target)) return;
    drag.target = t;
    if (t) {
        send("overlay", { kind: t.kind === "swap" || t.kind === "fill" ? "move" : t.kind });
        showOverlay(t.g.cells[t.j]);
    } else {
        hideOverlay();
    }
}

function dragFinished(w) {
    if (!drag || drag.w !== w) return;
    const d = drag;
    drag = null;
    try {
        // no step at all (a click on the title bar) counts as staying put
        const t = d.resizing === false ? dropTarget(d, cursorPoint()) : null;
        applyDrop(d, w, t);
    } finally {
        // whatever happened, never leave the highlight on screen
        if (d.target) hideOverlay();
    }
}

function applyDrop(d, w, t) {
    // the group may have closed during the drag
    const from = d.from && groups.indexOf(d.from.g) >= 0 && d.from.g.slots[d.from.i].w === w ? d.from : null;
    if (from && (!t || t.kind === "stay")) {
        placeSlot(from.g, from.i);
        return;
    }
    if (!t || groups.indexOf(t.g) < 0) return;
    if (from && t.kind === "leave") {
        release(from.g, from.i, true, false);
        userChanged.add(from.g.id);
    } else if (from && (t.kind === "swap" || t.kind === "fill")) {
        const a = from.g,
            i = from.i,
            b = t.g,
            j = t.j;
        if (a === b) {
            const tmp = a.slots[i];
            a.slots[i] = a.slots[j];
            a.slots[j] = tmp;
            placeSlot(a, i);
            placeSlot(a, j);
        } else {
            const other = b.slots[j].w;
            release(a, i, false, false);
            release(b, j, false, false);
            adopt(b, j, w);
            if (other) adopt(a, i, other);
        }
        userChanged.add(a.id);
        userChanged.add(b.id);
    } else if (!from && t.kind === "fill" && !t.g.slots[t.j].w) {
        adopt(t.g, t.j, w);
        if (!t.g.minimized) quiet(() => workspace.raiseWindow(w));
        userChanged.add(t.g.id);
    }
    stateChanged();
}

// ------------------------------------------------------------------ window events

function hook(w) {
    w.minimizedChanged.connect(() => onMinimized(w));
    w.maximizedChanged.connect(() => {
        if (guard) return;
        const loc = locate(w);
        if (loc && w.maximizeMode !== 0 && !loc.g.minimized) placeSlot(loc.g, loc.i);
    });
    w.fullScreenChanged.connect(() => {
        if (guard) return;
        const loc = locate(w);
        if (loc && !w.fullScreen && !loc.g.minimized) placeSlot(loc.g, loc.i); // back from a video
    });
    w.frameGeometryChanged.connect(() => {
        if (guard || drag || w.move || w.resize) return;
        const loc = locate(w);
        if (!loc || loc.g.minimized || w.fullScreen) return;
        const s = loc.g.slots[loc.i];
        // apps that restore their own size/position right after starting
        if (s.settle && Date.now() < s.settle.until && s.settle.tries < SETTLE_TRIES) {
            const c = loc.g.cells[loc.i],
                f = w.frameGeometry;
            if (Math.abs(f.x - c.x) + Math.abs(f.y - c.y) + Math.abs(f.width - c.width) + Math.abs(f.height - c.height) > 4) {
                s.settle.tries++;
                placeSlot(loc.g, loc.i);
            }
        }
    });
    w.interactiveMoveResizeStarted.connect(() => dragStarted(w));
    w.interactiveMoveResizeStepped.connect(() => dragStepped(w));
    w.interactiveMoveResizeFinished.connect(() => dragFinished(w));
    w.captionChanged.connect(() => {
        if (isOurs(w)) classify(w);
        else if (locate(w)) stateChanged();
        else catchLaunched(w);
    });
    w.desktopFileNameChanged.connect(() => {
        if (isOurs(w)) classify(w);
        else catchLaunched(w);
    });
}

function onMinimized(w) {
    if (guard) return;
    for (const g of groups) {
        if (g.proxy === w) {
            // a click on the group's taskbar button (minimize when active / restore)
            proxyClicked(g);
            return;
        }
    }
    const loc = locate(w);
    if (!loc) return;
    const g = loc.g;
    if (w.minimized && !g.minimized) minimizeGroup(g); // its own button is gone: take the group down
    else if (!w.minimized && g.minimized) showGroup(g, false); // e.g. picked in Alt+Tab
}

function onActivated(w) {
    if (!w || guard) return;
    for (const g of groups) {
        if (g.proxy === w && !w.minimized) {
            proxyClicked(g);
            return;
        }
    }
}

function onRemoved(w) {
    if (w === overlay) overlay = null;
    orphans = orphans.filter((x) => x !== w);
    origSkip.delete(wid(w));
    if (drag && drag.w === w) {
        if (drag.target) {
            send("overlay", { kind: null });
            idleOverlay();
        }
        drag = null;
    }
    for (const g of groups.slice()) {
        if (g.proxy === w) {
            // the app closed (or crashed): give every window back
            closeGroup(g, true);
            continue;
        }
        const pi = g.plus.indexOf(w);
        if (pi >= 0) g.plus[pi] = null;
        const i = g.slots.findIndex((s) => s.w === w);
        if (i < 0) continue;
        const s = g.slots[i];
        const rearm = s.armedUntil > Date.now() && s.spec ? s.spec : null;
        release(g, i, false, true);
        if (rearm) {
            // probably a splash screen: keep waiting for the real window
            pending.push({ gid: g.id, cell: i, spec: rearm, before: new Set(workspace.windowList().map(wid)), deadline: s.armedUntil || Date.now() + LAUNCH_WAIT });
        }
        stateChanged();
    }
}

function relayoutAll() {
    sendOutputs();
    for (const g of groups) {
        layoutGroup(g);
        if (g.minimized) continue;
        g.slots.forEach((s, i) => placeSlot(g, i));
        g.plus.forEach((p, i) => placePlus(g, i));
    }
}

// ------------------------------------------------------------------ start

for (const w of workspace.windowList()) hook(w);
workspace.windowAdded.connect((w) => {
    hook(w);
    if (isOurs(w)) classify(w);
    else catchLaunched(w);
});
workspace.windowRemoved.connect(onRemoved);
workspace.windowActivated.connect(onActivated);
workspace.screensChanged.connect(relayoutAll);
workspace.virtualScreenGeometryChanged.connect(relayoutAll);

send("hello", { version: 1 });
poll();
