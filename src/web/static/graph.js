/* ═══════════════════════════════════════════════════════════
   recall — web dashboard logic
   ═══════════════════════════════════════════════════════════ */

let sessions = [];
let stats = {};
let currentView = 'timeline';

// Graph
const canvas = document.getElementById('graph');
const ctx = canvas ? canvas.getContext('2d') : null;
const tooltip = document.getElementById('tooltip');
let gNodes = [], gEdges = [];
let gW = 0, gH = 0;
let dragging = null, hoveredNode = null;

const C = {
    session: '#5eadfc',
    repo:    '#42d77d',
    temp:    'rgba(37,44,58,.5)',
    link:    'rgba(94,173,252,.18)',
};

// ─── Boot ────────────────────────────────────────────────
document.addEventListener('DOMContentLoaded', () => {
    bindTabs();
    bindSearch();
    bindDrawer();
    bindKeys();
    loadStats();
    loadSessions();
});

// ─── Tabs ────────────────────────────────────────────────
function bindTabs() {
    document.querySelectorAll('.nav-tab').forEach(t => {
        t.addEventListener('click', () => switchView(t.dataset.view));
    });
}

function switchView(v) {
    currentView = v;
    document.querySelectorAll('.nav-tab').forEach(t =>
        t.classList.toggle('active', t.dataset.view === v));
    document.querySelectorAll('.view').forEach(el =>
        el.classList.toggle('active', el.id === 'view-' + v));
    if (v === 'graph' && gNodes.length === 0) loadGraph();
}

// ─── Stats ───────────────────────────────────────────────
async function loadStats() {
    try {
        const r = await fetch('/api/stats');
        stats = await r.json();
        qs('#stat-sessions').textContent = num(stats.sessions);
        qs('#stat-commands').textContent = num(stats.commands);
        qs('#stat-repos').textContent    = num(stats.repos);
        qs('#stat-failures').textContent = num(stats.failures);
    } catch(e) { console.error('stats', e); }
}

// ─── Sessions ────────────────────────────────────────────
async function loadSessions() {
    try {
        const r = await fetch('/api/sessions?limit=200');
        const d = await r.json();
        sessions = d.sessions || [];
        renderTimeline();
    } catch(e) {
        console.error('sessions', e);
        qs('#timeline').innerHTML = emptyHTML();
    }
}

function renderTimeline() {
    const el = qs('#timeline');
    if (!sessions.length) { el.innerHTML = emptyHTML(); return; }

    const groups = {};
    sessions.forEach(s => {
        const key = new Date(s.start_time).toISOString().slice(0, 10);
        (groups[key] = groups[key] || []).push(s);
    });

    const today = isoDay(0), yesterday = isoDay(-1);
    let h = '';

    Object.entries(groups).forEach(([date, items]) => {
        let label = date;
        if (date === today) label = 'Today';
        else if (date === yesterday) label = 'Yesterday';
        else {
            const d = new Date(date + 'T12:00:00');
            label = d.toLocaleDateString('en-US', { weekday:'short', month:'short', day:'numeric' });
        }

        h += `<div class="day-group"><div class="day-label">${label}</div>`;

        items.forEach(s => {
            const time = new Date(s.start_time).toLocaleTimeString('en-US', { hour:'2-digit', minute:'2-digit', hour12:false });
            const term = s.terminal_app || '';
            const dir  = lastSegment(s.initial_dir) || '~';
            const cnt  = `${s.command_count} cmd${s.command_count !== 1 ? 's' : ''}`;

            let tags = '';
            (s.repos || []).forEach(r => {
                tags += `<span class="tag repo">${esc(lastSegment(r))}</span>`;
            });
            (s.branches || []).forEach(b => {
                tags += `<span class="tag branch">${esc(b)}</span>`;
            });

            const dot = s.has_failures ? '<span class="fail-dot"></span>' : '';

            h += `<div class="session-card" onclick="openSession('${esc(s.id)}')">
                <span class="s-time">${time}</span>
                <span class="s-term">${esc(term)}</span>
                <span class="s-dir">${esc(dir)}</span>
                <span class="s-count">${cnt}</span>
                <span class="s-tags">${tags}</span>
                <span class="s-status">${dot}</span>
            </div>`;
        });

        h += '</div>';
    });

    el.innerHTML = h;
}

// ─── Drawer ──────────────────────────────────────────────
function bindDrawer() {
    qs('#drawer-dim').addEventListener('click', closeDrawer);
    qs('#drawer-close').addEventListener('click', closeDrawer);
}

window.openSession = async function(id) {
    const s = sessions.find(x => x.id === id);
    if (!s) return;

    qs('#drawer-title').textContent = `Session ${id.slice(0, 8)}`;
    let meta = new Date(s.start_time).toLocaleString();
    if (s.terminal_app) meta += ` \u00b7 ${s.terminal_app}`;
    if (s.initial_dir)  meta += ` \u00b7 ${s.initial_dir}`;
    qs('#drawer-meta').textContent = meta;

    const body = qs('#drawer-body');
    body.innerHTML = '<div style="padding:28px;color:var(--c-muted)">Loading\u2026</div>';
    qs('#drawer').classList.remove('hidden');

    try {
        const r = await fetch(`/api/commands?session_id=${encodeURIComponent(id)}`);
        const d = await r.json();
        renderCmds(body, d.commands || []);
    } catch(e) {
        body.innerHTML = '<div style="padding:28px;color:var(--c-red)">Failed to load</div>';
    }
};

function renderCmds(el, cmds) {
    if (!cmds.length) { el.innerHTML = '<div style="padding:28px;color:var(--c-muted)">No commands</div>'; return; }

    let h = '';
    cmds.forEach(c => {
        const ts = new Date(c.timestamp).toLocaleTimeString('en-US', { hour:'2-digit', minute:'2-digit', second:'2-digit', hour12:false });
        const dur = fmtDur(c.duration_ms);
        const isFail = c.exit_code != null && c.exit_code !== 0;

        let icon, cls;
        if (c.exit_code === 0) { icon = '\u2713'; cls = 'ok'; }
        else if (c.exit_code != null) { icon = '\u2717'; cls = 'fail'; }
        else { icon = '\u2022'; cls = 'unk'; }

        h += `<div class="cmd">
            <span class="cmd-icon ${cls}">${icon}</span>
            <span class="cmd-ts">${ts}</span>
            <span class="cmd-dur">${dur}</span>
            <span class="cmd-txt${isFail ? ' err' : ''}">${esc(c.command_text)}</span>
        </div>`;

        if (c.git_repo || c.git_branch) {
            const repo = lastSegment(c.git_repo) || '';
            const br   = c.git_branch || '';
            const lbl  = repo && br ? `${repo}:${br}` : repo || br;
            h += `<div class="cmd-git-row"><span class="tag repo">${esc(lbl)}</span></div>`;
        }
    });
    el.innerHTML = h;
}

function closeDrawer() { qs('#drawer').classList.add('hidden'); }

// ─── Search ──────────────────────────────────────────────
function bindSearch() {
    const inp = qs('#search-input');
    let timer = null;

    inp.addEventListener('input', () => {
        clearTimeout(timer);
        const q = inp.value.trim();
        if (!q) { hideSearch(); return; }
        timer = setTimeout(() => doSearch(q), 250);
    });

    inp.addEventListener('keydown', e => {
        if (e.key === 'Escape') { inp.value = ''; inp.blur(); hideSearch(); }
        if (e.key === 'Enter') { clearTimeout(timer); const q = inp.value.trim(); if (q) doSearch(q); }
    });

    qs('#search-close').addEventListener('click', () => { inp.value = ''; hideSearch(); });
}

async function doSearch(q) {
    const overlay = qs('#search-overlay');
    const list = qs('#search-list');
    overlay.classList.remove('hidden');

    try {
        const r = await fetch(`/api/search?q=${encodeURIComponent(q)}&limit=50`);
        const d = await r.json();
        const results = d.results || [];

        qs('#search-count').textContent = `${results.length} result${results.length !== 1 ? 's' : ''} for \u201c${q}\u201d`;

        if (!results.length) {
            list.innerHTML = `<div class="empty"><p>No commands match \u201c${esc(q)}\u201d</p></div>`;
            return;
        }

        let h = '';
        results.forEach(c => {
            const ts = shortDate(c.timestamp);
            const dir = lastSegment(c.cwd) || '';
            const isFail = c.exit_code != null && c.exit_code !== 0;
            let icon, cls;
            if (c.exit_code === 0) { icon = '\u2713'; cls = 'ok'; }
            else if (c.exit_code != null) { icon = '\u2717'; cls = 'fail'; }
            else { icon = '\u2022'; cls = 'unk'; }

            const repo = c.git_repo ? `<span class="tag repo">${esc(lastSegment(c.git_repo))}</span>` : '';

            h += `<div class="sr-row">
                <span class="cmd-icon ${cls}">${icon}</span>
                <span class="cmd-ts">${ts}</span>
                <span class="s-dir" style="font-size:12px">${esc(dir)}</span>
                <span class="cmd-txt${isFail ? ' err' : ''}" style="font-size:13px">${esc(c.command_text)}</span>
                <span>${repo}</span>
            </div>`;
        });
        list.innerHTML = h;
    } catch(e) {
        list.innerHTML = '<div class="empty"><p>Search failed</p></div>';
    }
}

function hideSearch() { qs('#search-overlay').classList.add('hidden'); }

// ─── Keys ────────────────────────────────────────────────
function bindKeys() {
    document.addEventListener('keydown', e => {
        if (e.key === '/' && document.activeElement.tagName !== 'INPUT') {
            e.preventDefault(); qs('#search-input').focus();
        }
        if (e.key === 'Escape') { closeDrawer(); hideSearch(); }
    });
}

// ─── Graph ───────────────────────────────────────────────
function gResize() {
    if (!canvas) return;
    const wrap = canvas.parentElement;
    const rect = wrap.getBoundingClientRect();
    const dpr = devicePixelRatio || 1;
    gW = rect.width;
    gH = rect.height;
    canvas.width = gW * dpr;
    canvas.height = gH * dpr;
    canvas.style.width = gW + 'px';
    canvas.style.height = gH + 'px';
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}

async function loadGraph() {
    if (!canvas) return;
    gResize();
    try {
        const r = await fetch('/api/graph');
        const d = await r.json();

        // Node radius scales with command count
        const maxCmds = Math.max(1, ...d.nodes.map(n => n.commands || 1));

        gNodes = (d.nodes || []).map(n => {
            const scale = Math.sqrt((n.commands || 1) / maxCmds);
            return {
                ...n,
                x: gW/2 + (Math.random()-.5)*Math.min(gW,500),
                y: gH/2 + (Math.random()-.5)*Math.min(gH,350),
                vx: 0, vy: 0,
                radius: 10 + scale * 30,
            };
        });

        gEdges = (d.edges || []).map(e => ({
            si: gNodes.findIndex(n => n.id === e.source),
            ti: gNodes.findIndex(n => n.id === e.target),
            weight: e.shared_sessions || 1,
        })).filter(e => e.si >= 0 && e.ti >= 0);

        qs('#graph-info').textContent = `${gNodes.length} repos \u00b7 ${gEdges.length} connections`;
        bindGraphEvents();
        gTick();
    } catch(e) {
        qs('#graph-info').textContent = 'Failed to load';
        console.error(e);
    }
}

function gSim() {
    const rep = 2200, k = 0.007, damp = 0.87, cen = 0.003;

    for (let i = 0; i < gNodes.length; i++) {
        for (let j = i+1; j < gNodes.length; j++) {
            const a = gNodes[i], b = gNodes[j];
            const dx = b.x - a.x, dy = b.y - a.y;
            const d = Math.sqrt(dx*dx + dy*dy) || 1;
            const f = rep / (d*d);
            const fx = dx/d*f, fy = dy/d*f;
            a.vx -= fx; a.vy -= fy;
            b.vx += fx; b.vy += fy;
        }
    }

    gEdges.forEach(e => {
        const a = gNodes[e.si], b = gNodes[e.ti];
        if (!a || !b) return;
        const dx = b.x-a.x, dy = b.y-a.y;
        const d = Math.sqrt(dx*dx+dy*dy) || 1;
        const f = (d-110)*k;
        const fx = dx/d*f, fy = dy/d*f;
        a.vx += fx; a.vy += fy;
        b.vx -= fx; b.vy -= fy;
    });

    gNodes.forEach(n => {
        n.vx += (gW/2-n.x)*cen;
        n.vy += (gH/2-n.y)*cen;
        if (n === dragging) return;
        n.vx *= damp; n.vy *= damp;
        n.x += n.vx; n.y += n.vy;
        n.x = Math.max(n.radius, Math.min(gW-n.radius, n.x));
        n.y = Math.max(n.radius, Math.min(gH-n.radius, n.y));
    });
}

function gDraw() {
    ctx.clearRect(0, 0, gW, gH);

    // Edges — width scales with shared session count
    gEdges.forEach(e => {
        const a = gNodes[e.si], b = gNodes[e.ti];
        if (!a || !b) return;
        ctx.beginPath();
        ctx.moveTo(a.x, a.y); ctx.lineTo(b.x, b.y);
        ctx.strokeStyle = 'rgba(94, 173, 252, 0.25)';
        ctx.lineWidth = Math.min(e.weight * 1.5, 6);
        ctx.stroke();
    });

    // Nodes — green circles, sized by command count
    gNodes.forEach(n => {
        // Glow ring for hovered
        if (n === hoveredNode) {
            ctx.beginPath();
            ctx.arc(n.x, n.y, n.radius + 4, 0, Math.PI*2);
            ctx.strokeStyle = 'rgba(66, 215, 125, 0.4)';
            ctx.lineWidth = 2;
            ctx.stroke();
        }

        ctx.beginPath();
        ctx.arc(n.x, n.y, n.radius, 0, Math.PI*2);

        // Fill with gradient feel: failures = red tint
        const failRatio = n.failures / Math.max(1, n.commands);
        if (n === hoveredNode) {
            ctx.fillStyle = '#dce4f0';
            ctx.shadowColor = '#42d77d';
            ctx.shadowBlur = 20;
        } else if (failRatio > 0.3) {
            ctx.fillStyle = '#f46d6d';
            ctx.shadowBlur = 0;
        } else {
            ctx.fillStyle = '#42d77d';
            ctx.shadowBlur = 0;
        }
        ctx.fill();
        ctx.shadowBlur = 0;

        // Label
        const fontSize = Math.max(11, Math.min(14, n.radius * 0.45));
        ctx.font = `600 ${fontSize}px Inter, sans-serif`;
        ctx.fillStyle = '#dce4f0';
        ctx.textAlign = 'center';
        ctx.fillText(n.label, n.x, n.y + n.radius + 16);

        // Command count inside large nodes
        if (n.radius > 18) {
            ctx.font = `600 ${Math.floor(n.radius * 0.5)}px var(--mono), monospace`;
            ctx.fillStyle = 'rgba(10, 14, 20, 0.7)';
            ctx.fillText(n.commands, n.x, n.y + n.radius * 0.18);
        }
    });
}

function gFind(x, y) {
    for (const n of gNodes) {
        const dx = x-n.x, dy = y-n.y;
        if (dx*dx + dy*dy < (n.radius+5)**2) return n;
    }
    return null;
}

function gTick() {
    if (currentView !== 'graph') return;
    gSim(); gDraw();
    requestAnimationFrame(gTick);
}

function bindGraphEvents() {
    canvas.addEventListener('mousemove', e => {
        const r = canvas.getBoundingClientRect();
        const mx = e.clientX - r.left, my = e.clientY - r.top;
        if (dragging) { dragging.x = mx; dragging.y = my; dragging.vx = 0; dragging.vy = 0; return; }
        hoveredNode = gFind(mx, my);
        if (hoveredNode) {
            canvas.style.cursor = 'grab';
            tooltip.classList.remove('hidden');
            tooltip.style.left = (e.clientX+14)+'px';
            tooltip.style.top  = (e.clientY+14)+'px';
            let th = `<div class="t-name">${hoveredNode.label}</div>`;
            th += `<div class="t-info">${hoveredNode.commands} commands \u00b7 ${hoveredNode.sessions} sessions</div>`;
            if (hoveredNode.failures > 0) th += `<div class="t-info" style="color:#f46d6d">${hoveredNode.failures} failures</div>`;
            if (hoveredNode.branches && hoveredNode.branches.length) {
                th += `<div class="t-info">branches: ${hoveredNode.branches.join(', ')}</div>`;
            }
            if (hoveredNode.last_active) th += `<div class="t-info">last active: ${new Date(hoveredNode.last_active).toLocaleDateString()}</div>`;
            tooltip.innerHTML = th;
        } else {
            canvas.style.cursor = 'default';
            tooltip.classList.add('hidden');
        }
    });

    canvas.addEventListener('mousedown', e => {
        const r = canvas.getBoundingClientRect();
        const n = gFind(e.clientX - r.left, e.clientY - r.top);
        if (n) { dragging = n; canvas.style.cursor = 'grabbing'; }
    });

    canvas.addEventListener('mouseup', () => { dragging = null; canvas.style.cursor = hoveredNode ? 'grab' : 'default'; });
    canvas.addEventListener('mouseleave', () => { dragging = null; hoveredNode = null; tooltip.classList.add('hidden'); });

    qs('#btn-reset').addEventListener('click', () => {
        gNodes.forEach(n => {
            n.x = gW/2 + (Math.random()-.5)*Math.min(gW,500);
            n.y = gH/2 + (Math.random()-.5)*Math.min(gH,350);
            n.vx = 0; n.vy = 0;
        });
    });

    window.addEventListener('resize', () => { if (currentView === 'graph') gResize(); });
}

// ─── Helpers ─────────────────────────────────────────────
function qs(sel) { return document.querySelector(sel); }

function esc(s) {
    if (!s) return '';
    const d = document.createElement('div');
    d.textContent = s;
    return d.innerHTML;
}

function num(n) { return n == null ? '--' : n.toLocaleString(); }

function lastSegment(path) {
    if (!path) return '';
    return path.split('/').filter(Boolean).pop() || '';
}

function isoDay(offset) {
    const d = new Date();
    d.setDate(d.getDate() + offset);
    return d.toISOString().slice(0, 10);
}

function shortDate(ts) {
    const d = new Date(ts);
    return d.toLocaleDateString('en-US', { month:'short', day:'numeric' })
        + ' ' + d.toLocaleTimeString('en-US', { hour:'2-digit', minute:'2-digit', hour12:false });
}

function fmtDur(ms) {
    if (ms == null) return '-';
    if (ms >= 60000) return `${Math.floor(ms/60000)}m${Math.floor((ms%60000)/1000)}s`;
    if (ms >= 1000) return `${(ms/1000).toFixed(1)}s`;
    return `${ms}ms`;
}

function emptyHTML() {
    return `<div class="empty">
        <svg width="48" height="48" viewBox="0 0 144 144" fill="none" opacity=".3">
            <rect x="8" y="8" width="128" height="128" rx="20" stroke="currentColor" stroke-width="3"/>
            <polyline points="36,64 56,76 36,88" stroke="currentColor" stroke-width="5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>
            <line x1="64" y1="76" x2="108" y2="76" stroke="currentColor" stroke-width="4" stroke-linecap="round" opacity="0.7"/>
        </svg>
        <p>No sessions recorded yet.<br>Install the shell hook to start capturing:</p>
        <code>eval "$(recall init zsh)"</code>
    </div>`;
}
