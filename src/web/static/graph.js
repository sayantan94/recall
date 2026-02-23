/* ═══════════════════════════════════════════════════════════
   recall — web dashboard logic
   ═══════════════════════════════════════════════════════════ */

let sessions = [];
let stats = {};
let currentView = 'timeline';

// Repos
let repoNodes = [];
let repoSortBy = 'commands';

// Graph state
const canvas = document.getElementById('graph');
const ctx = canvas ? canvas.getContext('2d') : null;
let gNodes = [], gEdges = [], gParticles = [];
let gW = 0, gH = 0, gDpr = 1;
let gDragging = null, gHovered = null, gSelected = null;
let gPanX = 0, gPanY = 0, gZoom = 1;
let gMouseX = 0, gMouseY = 0;
let gLastMouse = { x: 0, y: 0 };
let gPanning = false;
let gFrame = 0;
let gLoaded = false;
let gSettled = false;
let gSettleFrame = 0;

// ─── Boot ────────────────────────────────────────────────
document.addEventListener('DOMContentLoaded', () => {
    bindTabs();
    bindSearch();
    bindDrawer();
    bindKeys();
    bindReposSort();
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
    if (v === 'repos' && repoNodes.length === 0) loadRepos();
    if (v === 'graph' && !gLoaded) loadGraph();
    if (v === 'graph') gTick();
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
            const dt = new Date(s.start_time);
            const time = dt.toLocaleDateString('en-US', { month:'short', day:'numeric' })
                + ' ' + dt.toLocaleTimeString('en-US', { hour:'2-digit', minute:'2-digit', hour12:false });
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

window.openToolSearch = async function(toolName) {
    qs('#drawer-title').textContent = toolName;
    qs('#drawer-meta').textContent = 'Loading commands\u2026';

    const body = qs('#drawer-body');
    body.innerHTML = '<div style="padding:28px;color:var(--c-muted)">Loading\u2026</div>';
    qs('#drawer').classList.remove('hidden');

    try {
        const r = await fetch(`/api/search?q=${encodeURIComponent(toolName)}&limit=100`);
        const d = await r.json();
        const cmds = (d.results || []).filter(c => {
            const firstToken = (c.command_text || '').split(/\s+/)[0] || '';
            const basename = firstToken.split('/').pop() || '';
            return basename.toLowerCase() === toolName.toLowerCase();
        });

        const node = gNodes.find(n => n.label === toolName && n.type === 'tool');
        let meta = '';
        if (node) {
            meta = `${node.commands} commands \u00b7 ${node.sessions} sessions`;
            if (node.failures > 0) meta += ` \u00b7 ${node.failures} failures`;
            if (node.repos && node.repos.length) meta += ` \u00b7 ${node.repos.length} repos`;
        }
        qs('#drawer-meta').textContent = meta;

        if (!cmds.length) {
            body.innerHTML = '<div style="padding:28px;color:var(--c-muted)">No commands found for this tool</div>';
            return;
        }
        renderCmds(body, cmds);
    } catch(e) {
        body.innerHTML = '<div style="padding:28px;color:var(--c-red)">Failed to load</div>';
    }
};

window.openRepo = async function(repoName) {
    qs('#drawer-title').textContent = repoName;
    qs('#drawer-meta').textContent = 'Loading commands\u2026';

    const body = qs('#drawer-body');
    body.innerHTML = '<div style="padding:28px;color:var(--c-muted)">Loading\u2026</div>';
    qs('#drawer').classList.remove('hidden');

    try {
        const r = await fetch(`/api/search?q=${encodeURIComponent(repoName)}&limit=100`);
        const d = await r.json();
        const cmds = (d.results || []).filter(c =>
            c.git_repo && lastSegment(c.git_repo) === repoName
        );

        const node = gNodes.find(n => n.label === repoName) || repoNodes.find(n => n.label === repoName);
        let meta = '';
        if (node) {
            meta = `${node.commands} commands \u00b7 ${node.sessions} sessions`;
            if (node.failures > 0) meta += ` \u00b7 ${node.failures} failures`;
        }
        qs('#drawer-meta').textContent = meta;

        if (!cmds.length) {
            body.innerHTML = '<div style="padding:28px;color:var(--c-muted)">No commands found for this repo</div>';
            return;
        }
        renderCmds(body, cmds);
    } catch(e) {
        body.innerHTML = '<div style="padding:28px;color:var(--c-red)">Failed to load</div>';
    }
};

let outputIdCounter = 0;
const outputStore = {};

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

        const oid = 'out-' + (outputIdCounter++);
        const hasOutput = c.output && c.output.trim();

        h += `<div class="cmd${hasOutput ? ' has-output' : ''}"${hasOutput ? ` onclick="toggleOutput('${oid}')"` : ''}>
            <span class="cmd-icon ${cls}">${icon}</span>
            <span class="cmd-ts">${ts}</span>
            <span class="cmd-dur">${dur}</span>
            <span class="cmd-txt${isFail ? ' err' : ''}">${esc(c.command_text)}${hasOutput ? '<span class="cmd-output-badge">output</span>' : ''}</span>
        </div>`;

        if (hasOutput) {
            const full = c.output.trim();
            const lines = full.split('\n');
            const preview = lines.slice(0, 5).join('\n');
            const hasMore = lines.length > 5;

            if (hasMore) outputStore[oid] = full;

            h += `<div class="cmd-output hidden" id="${oid}">
                <pre class="cmd-output-pre">${esc(preview)}</pre>
                ${hasMore ? `<button class="cmd-output-expand" onclick="event.stopPropagation(); expandOutput('${oid}')">Show all ${lines.length} lines</button>` : ''}
            </div>`;
        }

        if (c.git_repo || c.git_branch) {
            const repo = lastSegment(c.git_repo) || '';
            const br   = c.git_branch || '';
            const lbl  = repo && br ? `${repo}:${br}` : repo || br;
            h += `<div class="cmd-git-row"><span class="tag repo">${esc(lbl)}</span></div>`;
        }
    });
    el.innerHTML = h;
}

window.toggleOutput = function(id) {
    const el = document.getElementById(id);
    if (el) el.classList.toggle('hidden');
};

window.expandOutput = function(id) {
    const wrap = document.getElementById(id);
    if (!wrap) return;
    const pre = wrap.querySelector('.cmd-output-pre');
    const btn = wrap.querySelector('.cmd-output-expand');
    if (pre && outputStore[id]) pre.textContent = outputStore[id];
    if (btn) btn.remove();
};

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

// ─── Repos ──────────────────────────────────────────────
function bindReposSort() {
    const sel = qs('#repos-sort-select');
    if (sel) {
        sel.addEventListener('change', () => {
            repoSortBy = sel.value;
            renderRepos();
        });
    }
}

async function loadRepos() {
    try {
        const r = await fetch('/api/graph');
        const d = await r.json();
        repoNodes = (d.nodes || []).filter(n => n.type !== 'tool');
        qs('#repos-info').textContent = `${repoNodes.length} repo${repoNodes.length !== 1 ? 's' : ''}`;
        renderRepos();
    } catch(e) {
        qs('#repos-info').textContent = 'Failed to load';
        console.error(e);
    }
}

function renderRepos() {
    const grid = qs('#repos-grid');
    if (!repoNodes.length) {
        grid.innerHTML = '<div class="empty"><p>No repos recorded yet.</p></div>';
        return;
    }

    const maxCmds = Math.max(1, ...repoNodes.map(n => n.commands || 0));

    const sorted = [...repoNodes].sort((a, b) => {
        switch (repoSortBy) {
            case 'commands': return (b.commands || 0) - (a.commands || 0);
            case 'recent':   return (b.last_active || 0) - (a.last_active || 0);
            case 'failures': return (b.failures || 0) - (a.failures || 0);
            case 'name':     return (a.label || '').localeCompare(b.label || '');
            default:         return 0;
        }
    });

    let h = '';
    sorted.forEach(n => {
        const failRate = n.commands > 0 ? n.failures / n.commands : 0;
        const barPct = Math.max(2, Math.round((n.commands / maxCmds) * 100));
        const barClass = failRate > 0.3 ? 'danger' : failRate > 0.1 ? 'warn' : '';

        const lastActive = n.last_active
            ? new Date(n.last_active).toLocaleDateString('en-US', { month:'short', day:'numeric', year:'numeric' })
              + ' ' + new Date(n.last_active).toLocaleTimeString('en-US', { hour:'2-digit', minute:'2-digit', hour12:false })
            : 'Unknown';

        let branchTags = '';
        if (n.branches && n.branches.length) {
            n.branches.slice(0, 5).forEach(b => {
                branchTags += `<span class="tag branch">${esc(b)}</span>`;
            });
            if (n.branches.length > 5) {
                branchTags += `<span class="tag branch">+${n.branches.length - 5}</span>`;
            }
        }

        h += `<div class="repo-card" onclick="openRepo('${esc(n.label)}')">
            <div class="repo-card-header">
                <span class="repo-card-name">${esc(n.label)}</span>
                <span class="repo-card-active">${lastActive}</span>
            </div>
            <div class="repo-card-stats">
                <div class="repo-stat">
                    <span class="repo-stat-num">${num(n.commands)}</span>
                    <span class="repo-stat-lbl">Commands</span>
                </div>
                <div class="repo-stat">
                    <span class="repo-stat-num">${num(n.sessions)}</span>
                    <span class="repo-stat-lbl">Sessions</span>
                </div>
                <div class="repo-stat">
                    <span class="repo-stat-num${n.failures > 0 ? ' red' : ''}">${num(n.failures)}</span>
                    <span class="repo-stat-lbl">Failures</span>
                </div>
            </div>
            <div class="repo-card-bar">
                <div class="repo-card-bar-fill ${barClass}" style="width:${barPct}%"></div>
            </div>
            ${branchTags ? `<div class="repo-card-branches">${branchTags}</div>` : ''}
        </div>`;
    });

    grid.innerHTML = h;
}

// ═════════════════════════════════════════════════════════
//  GRAPH — force-directed with glow, particles, pan/zoom
// ═════════════════════════════════════════════════════════

// Color palette for repo nodes — hue-shifted by index for variety
const NODE_COLORS = [
    { core: '#42d77d', glow: 'rgba(66,215,125,' },   // green
    { core: '#5eadfc', glow: 'rgba(94,173,252,' },   // blue
    { core: '#c791f7', glow: 'rgba(199,145,247,' },  // purple
    { core: '#5ee7d5', glow: 'rgba(94,231,213,' },   // cyan
    { core: '#e8b84b', glow: 'rgba(232,184,75,' },   // amber
    { core: '#f46d6d', glow: 'rgba(244,109,109,' },  // red
    { core: '#f7a05e', glow: 'rgba(247,160,94,' },   // orange
    { core: '#7cbafc', glow: 'rgba(124,186,252,' },  // light blue
];

// Color palette for tool nodes — blue/cyan tones
const TOOL_COLORS = [
    { core: '#4ecdc4', glow: 'rgba(78,205,196,' },   // teal
    { core: '#45b7d1', glow: 'rgba(69,183,209,' },   // sky
    { core: '#6cb4ee', glow: 'rgba(108,180,238,' },  // soft blue
    { core: '#96e6a1', glow: 'rgba(150,230,161,' },  // mint
    { core: '#7fd8be', glow: 'rgba(127,216,190,' },  // seafoam
    { core: '#a8d8ea', glow: 'rgba(168,216,234,' },  // ice
];

function nodeColor(n, idx) {
    const failRate = n.failures / Math.max(1, n.commands);
    if (failRate > 0.3) return { core: '#f46d6d', glow: 'rgba(244,109,109,' };
    if (n.type === 'tool') return TOOL_COLORS[idx % TOOL_COLORS.length];
    return NODE_COLORS[idx % NODE_COLORS.length];
}

function gResize() {
    if (!canvas) return;
    const wrap = canvas.parentElement;
    const rect = wrap.getBoundingClientRect();
    gDpr = devicePixelRatio || 1;
    gW = rect.width;
    gH = rect.height;
    canvas.width = gW * gDpr;
    canvas.height = gH * gDpr;
    canvas.style.width = gW + 'px';
    canvas.style.height = gH + 'px';
    ctx.setTransform(gDpr, 0, 0, gDpr, 0, 0);
}

// Convert screen coords to world coords
function screenToWorld(sx, sy) {
    return { x: (sx - gPanX) / gZoom, y: (sy - gPanY) / gZoom };
}

function worldToScreen(wx, wy) {
    return { x: wx * gZoom + gPanX, y: wy * gZoom + gPanY };
}

async function loadGraph() {
    if (!canvas) return;
    gLoaded = true;
    gResize();

    try {
        const r = await fetch('/api/graph');
        const d = await r.json();

        const allNodes = d.nodes || [];
        const repoNodesRaw = allNodes.filter(n => n.type === 'repo');
        const toolNodesRaw = allNodes.filter(n => n.type === 'tool');

        const maxRepoCmds = Math.max(1, ...repoNodesRaw.map(n => n.commands || 1));
        const maxToolCmds = Math.max(1, ...toolNodesRaw.map(n => n.commands || 1));

        let repoIdx = 0, toolIdx = 0;
        gNodes = allNodes.map((n) => {
            let radius, idx;
            if (n.type === 'tool') {
                const scale = Math.sqrt((n.commands || 1) / maxToolCmds);
                radius = 8 + scale * 14; // 8-22px for tools
                idx = toolIdx++;
            } else {
                const scale = Math.sqrt((n.commands || 1) / maxRepoCmds);
                radius = 14 + scale * 36; // 14-50px for repos
                idx = repoIdx++;
            }
            return {
                ...n,
                x: (Math.random() - 0.5) * Math.min(gW, 800),
                y: (Math.random() - 0.5) * Math.min(gH, 600),
                vx: 0, vy: 0,
                radius,
                color: nodeColor(n, idx),
                idx,
                pulse: Math.random() * Math.PI * 2,
            };
        });

        gEdges = (d.edges || []).map(e => ({
            si: gNodes.findIndex(n => n.id === e.source),
            ti: gNodes.findIndex(n => n.id === e.target),
            weight: e.shared_sessions || e.weight || 1,
            type: e.type || 'repo-repo',
        })).filter(e => e.si >= 0 && e.ti >= 0);

        // Spawn initial particles on edges
        gParticles = [];
        gEdges.forEach((e, ei) => {
            const count = Math.min(e.weight, 4);
            for (let i = 0; i < count; i++) {
                gParticles.push({ ei, t: Math.random(), speed: 0.002 + Math.random() * 0.003 });
            }
        });

        // Center pan
        gPanX = gW / 2;
        gPanY = gH / 2;
        gZoom = 1;
        gSettled = false;
        gSettleFrame = 0;

        bindGraphEvents();
        gTick();
    } catch(e) {
        console.error('graph load', e);
    }
}

// ─── Physics ─────────────────────────────────────────────
function gSim() {
    if (gSettled) return;

    const rep = 3000;
    const springK = 0.005;
    const springLenRepoRepo = 180;
    const springLenRepoTool = 100;
    const damp = 0.88;
    const cen = 0.002;

    // Repulsion (all pairs)
    for (let i = 0; i < gNodes.length; i++) {
        for (let j = i + 1; j < gNodes.length; j++) {
            const a = gNodes[i], b = gNodes[j];
            // Weaker repulsion between tool nodes so they cluster closer
            const bothTools = a.type === 'tool' && b.type === 'tool';
            const r = bothTools ? rep * 0.3 : rep;
            const dx = b.x - a.x, dy = b.y - a.y;
            const d = Math.sqrt(dx * dx + dy * dy) || 1;
            const f = r / (d * d);
            const fx = (dx / d) * f, fy = (dy / d) * f;
            a.vx -= fx; a.vy -= fy;
            b.vx += fx; b.vy += fy;
        }
    }

    // Spring attraction (edges)
    gEdges.forEach(e => {
        const a = gNodes[e.si], b = gNodes[e.ti];
        if (!a || !b) return;
        const springLen = e.type === 'repo-tool' ? springLenRepoTool : springLenRepoRepo;
        const dx = b.x - a.x, dy = b.y - a.y;
        const d = Math.sqrt(dx * dx + dy * dy) || 1;
        const f = (d - springLen) * springK;
        const fx = (dx / d) * f, fy = (dy / d) * f;
        a.vx += fx; a.vy += fy;
        b.vx -= fx; b.vy -= fy;
    });

    // Centering + damping + integration
    let totalV = 0;
    gNodes.forEach(n => {
        n.vx -= n.x * cen;
        n.vy -= n.y * cen;
        if (n === gDragging) { n.vx = 0; n.vy = 0; return; }
        n.vx *= damp;
        n.vy *= damp;
        n.x += n.vx;
        n.y += n.vy;
        totalV += Math.abs(n.vx) + Math.abs(n.vy);
    });

    // Settle after physics converge
    gSettleFrame++;
    if (gSettleFrame > 300 && totalV < 0.5) gSettled = true;
}

// ─── Render ──────────────────────────────────────────────
function gDraw() {
    gFrame++;
    ctx.clearRect(0, 0, gW, gH);

    // Background grid dots
    ctx.save();
    ctx.translate(gPanX, gPanY);
    ctx.scale(gZoom, gZoom);

    drawGrid();
    drawEdges();
    drawParticles();
    drawNodes();

    ctx.restore();
}

function drawGrid() {
    const spacing = 60;
    const viewLeft = -gPanX / gZoom;
    const viewTop = -gPanY / gZoom;
    const viewRight = (gW - gPanX) / gZoom;
    const viewBottom = (gH - gPanY) / gZoom;

    const startX = Math.floor(viewLeft / spacing) * spacing;
    const startY = Math.floor(viewTop / spacing) * spacing;

    ctx.fillStyle = 'rgba(94,173,252,0.04)';
    for (let x = startX; x < viewRight; x += spacing) {
        for (let y = startY; y < viewBottom; y += spacing) {
            ctx.beginPath();
            ctx.arc(x, y, 1, 0, Math.PI * 2);
            ctx.fill();
        }
    }
}

function drawEdges() {
    gEdges.forEach(e => {
        const a = gNodes[e.si], b = gNodes[e.ti];
        if (!a || !b) return;

        const isRepoTool = e.type === 'repo-tool';
        const isHighlight = gHovered && (gHovered === a || gHovered === b);
        const isSelected = gSelected && (gSelected === a || gSelected === b);

        let alpha, width;
        if (isRepoTool) {
            // Thinner, more transparent for supporting links
            alpha = isHighlight || isSelected ? 0.35 : 0.06;
            width = Math.min(0.5 + e.weight * 0.5, 3) * (isHighlight || isSelected ? 1.5 : 1);
        } else {
            // Thicker, bolder for primary connections
            alpha = isHighlight || isSelected ? 0.5 : 0.15;
            width = Math.min(1.5 + e.weight * 1.5, 7) * (isHighlight || isSelected ? 1.5 : 1);
        }

        // Edge gradient
        const grad = ctx.createLinearGradient(a.x, a.y, b.x, b.y);
        grad.addColorStop(0, a.color.glow + alpha + ')');
        grad.addColorStop(1, b.color.glow + alpha + ')');

        ctx.beginPath();
        ctx.moveTo(a.x, a.y);
        ctx.lineTo(b.x, b.y);
        ctx.strokeStyle = grad;
        ctx.lineWidth = width;
        if (isRepoTool) ctx.setLineDash([4, 4]);
        ctx.stroke();
        if (isRepoTool) ctx.setLineDash([]);
    });
}

function drawParticles() {
    gParticles.forEach(p => {
        const e = gEdges[p.ei];
        if (!e) return;
        const a = gNodes[e.si], b = gNodes[e.ti];
        if (!a || !b) return;

        p.t += p.speed;
        if (p.t > 1) p.t -= 1;

        const x = a.x + (b.x - a.x) * p.t;
        const y = a.y + (b.y - a.y) * p.t;

        const isHighlight = gHovered && (gHovered === a || gHovered === b);
        const alpha = isHighlight ? 0.9 : 0.5;
        const radius = isHighlight ? 2.5 : 1.5;

        ctx.beginPath();
        ctx.arc(x, y, radius, 0, Math.PI * 2);
        ctx.fillStyle = a.color.glow + alpha + ')';
        ctx.fill();
    });
}

function drawNodes() {
    // Sort: tools behind repos, hovered/selected on top
    const sorted = [...gNodes].sort((a, b) => {
        if (a === gHovered || a === gSelected) return 1;
        if (b === gHovered || b === gSelected) return -1;
        // Repos on top of tools
        if (a.type === 'repo' && b.type === 'tool') return 1;
        if (a.type === 'tool' && b.type === 'repo') return -1;
        return 0;
    });

    sorted.forEach(n => {
        const isHov = n === gHovered;
        const isSel = n === gSelected;
        const isActive = isHov || isSel;
        const isTool = n.type === 'tool';

        // Connected dimming: if something is hovered, dim unconnected nodes
        let isDimmed = false;
        if (gHovered && !isActive) {
            isDimmed = !gEdges.some(e =>
                (gNodes[e.si] === gHovered && gNodes[e.ti] === n) ||
                (gNodes[e.ti] === gHovered && gNodes[e.si] === n)
            );
        }

        const r = n.radius * (isActive ? 1.12 : 1);

        // Outer glow
        if (isActive) {
            const pulseR = r + (isTool ? 5 : 8) + Math.sin(gFrame * 0.06 + n.pulse) * 3;
            ctx.beginPath();
            ctx.arc(n.x, n.y, pulseR, 0, Math.PI * 2);
            ctx.fillStyle = n.color.glow + '0.08)';
            ctx.fill();

            ctx.beginPath();
            ctx.arc(n.x, n.y, r + 3, 0, Math.PI * 2);
            ctx.strokeStyle = n.color.glow + '0.4)';
            ctx.lineWidth = isTool ? 1.5 : 2;
            ctx.stroke();
        }

        // Ambient pulse (subtle)
        const ambPulse = Math.sin(gFrame * 0.02 + n.pulse) * 0.05 + 0.95;

        // Node body — tools are slightly translucent
        const toolAlphaMod = isTool ? 0.75 : 1;
        const grad = ctx.createRadialGradient(n.x - r * 0.3, n.y - r * 0.3, r * 0.1, n.x, n.y, r);
        const baseAlpha = isDimmed ? 0.2 : ambPulse * toolAlphaMod;
        grad.addColorStop(0, n.color.glow + Math.min(1, baseAlpha + 0.3) + ')');
        grad.addColorStop(0.7, n.color.glow + baseAlpha + ')');
        grad.addColorStop(1, n.color.glow + (baseAlpha * 0.6) + ')');

        ctx.beginPath();
        ctx.arc(n.x, n.y, r, 0, Math.PI * 2);
        ctx.fillStyle = grad;
        ctx.fill();

        // Dashed ring outline for tool nodes
        if (isTool && !isDimmed) {
            ctx.beginPath();
            ctx.arc(n.x, n.y, r + 1.5, 0, Math.PI * 2);
            ctx.strokeStyle = n.color.glow + '0.3)';
            ctx.lineWidth = 1;
            ctx.setLineDash([3, 3]);
            ctx.stroke();
            ctx.setLineDash([]);
        }

        // Inner shine
        if (!isDimmed) {
            const shine = ctx.createRadialGradient(n.x - r * 0.25, n.y - r * 0.3, 0, n.x, n.y, r);
            shine.addColorStop(0, 'rgba(255,255,255,0.15)');
            shine.addColorStop(0.5, 'rgba(255,255,255,0.02)');
            shine.addColorStop(1, 'rgba(255,255,255,0)');
            ctx.beginPath();
            ctx.arc(n.x, n.y, r, 0, Math.PI * 2);
            ctx.fillStyle = shine;
            ctx.fill();
        }

        // Label below
        const labelAlpha = isDimmed ? 0.3 : (isTool ? 0.8 : 1);
        const fontSize = isTool ? Math.max(9, Math.min(12, r * 0.55)) : Math.max(11, Math.min(15, r * 0.4));
        ctx.font = `${isTool ? '500' : '600'} ${fontSize}px 'Inter', sans-serif`;
        ctx.textAlign = 'center';
        ctx.fillStyle = `rgba(220,228,240,${labelAlpha})`;
        ctx.fillText(n.label, n.x, n.y + r + (isTool ? 14 : 18));

        // Command count inside node (repos only, when large enough)
        if (!isTool && r > 20 && !isDimmed) {
            const numSize = Math.floor(r * 0.42);
            ctx.font = `700 ${numSize}px 'JetBrains Mono', monospace`;
            ctx.fillStyle = 'rgba(10,14,20,0.55)';
            ctx.fillText(n.commands, n.x, n.y + numSize * 0.15);
        }
    });
}

// ─── Hit test ────────────────────────────────────────────
function gFind(sx, sy) {
    const w = screenToWorld(sx, sy);
    for (let i = gNodes.length - 1; i >= 0; i--) {
        const n = gNodes[i];
        const dx = w.x - n.x, dy = w.y - n.y;
        if (dx * dx + dy * dy < (n.radius + 8) ** 2) return n;
    }
    return null;
}

// ─── HUD (info panel) ───────────────────────────────────
function updateHUD(n) {
    const hud = qs('#graph-hud');
    if (!n) { hud.classList.add('hidden'); return; }

    hud.classList.remove('hidden');

    if (n.type === 'tool') {
        // Tool HUD
        let repoTags = '';
        if (n.repos && n.repos.length) {
            repoTags = '<div class="hud-branches">' +
                n.repos.slice(0, 6).map(r => `<span class="tag repo">${esc(r)}</span>`).join('') +
                (n.repos.length > 6 ? `<span class="tag repo">+${n.repos.length - 6}</span>` : '') +
                '</div>';
        }

        // Find connected tools for this repo (show in HUD)
        hud.innerHTML = `
            <div class="hud-name" style="opacity:0.7;font-size:11px;margin-bottom:2px">TOOL</div>
            <div class="hud-name">${esc(n.label)}</div>
            <div class="hud-stats">
                <div><span class="hud-stat-val">${n.commands}</span><div class="hud-stat-lbl">Commands</div></div>
                <div><span class="hud-stat-val">${n.sessions}</span><div class="hud-stat-lbl">Sessions</div></div>
                <div><span class="hud-stat-val${n.failures > 0 ? ' red' : ''}">${n.failures}</span><div class="hud-stat-lbl">Failures</div></div>
            </div>
            ${repoTags}
            <div class="hud-hint">Click to search commands</div>
        `;

        hud.onclick = () => openToolSearch(n.label);
    } else {
        // Repo HUD
        const lastDate = n.last_active
            ? new Date(n.last_active).toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' })
              + ' ' + new Date(n.last_active).toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit', hour12: false })
            : '';

        let branches = '';
        if (n.branches && n.branches.length) {
            branches = '<div class="hud-branches">' +
                n.branches.slice(0, 4).map(b => `<span class="tag branch">${esc(b)}</span>`).join('') +
                (n.branches.length > 4 ? `<span class="tag branch">+${n.branches.length - 4}</span>` : '') +
                '</div>';
        }

        // Show connected tools
        const connectedTools = gEdges
            .filter(e => e.type === 'repo-tool' && (gNodes[e.si] === n || gNodes[e.ti] === n))
            .map(e => gNodes[e.si] === n ? gNodes[e.ti] : gNodes[e.si])
            .filter(t => t && t.type === 'tool')
            .sort((a, b) => b.commands - a.commands)
            .slice(0, 8);

        let toolTags = '';
        if (connectedTools.length) {
            toolTags = '<div class="hud-branches" style="margin-top:6px">' +
                connectedTools.map(t => `<span class="tag branch" style="border-color:rgba(78,205,196,0.3);color:rgba(78,205,196,0.9)">${esc(t.label)}</span>`).join('') +
                '</div>';
        }

        hud.innerHTML = `
            <div class="hud-name">${esc(n.label)}</div>
            <div class="hud-stats">
                <div><span class="hud-stat-val">${n.commands}</span><div class="hud-stat-lbl">Commands</div></div>
                <div><span class="hud-stat-val">${n.sessions}</span><div class="hud-stat-lbl">Sessions</div></div>
                <div><span class="hud-stat-val${n.failures > 0 ? ' red' : ''}">${n.failures}</span><div class="hud-stat-lbl">Failures</div></div>
                <div><span class="hud-stat-val" style="font-size:12px">${lastDate}</span><div class="hud-stat-lbl">Last active</div></div>
            </div>
            ${branches}
            ${toolTags}
            <div class="hud-hint">Click to view commands</div>
        `;

        hud.onclick = () => openRepo(n.label);
    }
}

// ─── Animation loop ──────────────────────────────────────
function gTick() {
    if (currentView !== 'graph') return;
    gSim();
    gDraw();
    requestAnimationFrame(gTick);
}

// ─── Events ──────────────────────────────────────────────
function bindGraphEvents() {
    if (!canvas) return;

    canvas.addEventListener('mousemove', e => {
        const rect = canvas.getBoundingClientRect();
        const mx = e.clientX - rect.left, my = e.clientY - rect.top;
        gMouseX = mx; gMouseY = my;

        if (gDragging) {
            const w = screenToWorld(mx, my);
            gDragging.x = w.x;
            gDragging.y = w.y;
            gDragging.vx = 0;
            gDragging.vy = 0;
            gSettled = false;
            gSettleFrame = 0;
            return;
        }

        if (gPanning) {
            gPanX += mx - gLastMouse.x;
            gPanY += my - gLastMouse.y;
            gLastMouse.x = mx;
            gLastMouse.y = my;
            return;
        }

        const found = gFind(mx, my);
        if (found !== gHovered) {
            gHovered = found;
            canvas.style.cursor = found ? 'pointer' : 'grab';
            updateHUD(found || gSelected);
        }
    });

    canvas.addEventListener('mousedown', e => {
        const rect = canvas.getBoundingClientRect();
        const mx = e.clientX - rect.left, my = e.clientY - rect.top;
        const n = gFind(mx, my);

        if (n) {
            gDragging = n;
            canvas.style.cursor = 'grabbing';
        } else {
            gPanning = true;
            gLastMouse.x = mx;
            gLastMouse.y = my;
            canvas.style.cursor = 'grabbing';
        }
    });

    canvas.addEventListener('mouseup', e => {
        if (gDragging) {
            // If barely moved, treat as click -> select
            const rect = canvas.getBoundingClientRect();
            const mx = e.clientX - rect.left, my = e.clientY - rect.top;
            const n = gFind(mx, my);
            if (n && n === gDragging) {
                gSelected = (gSelected === n) ? null : n;
                updateHUD(gSelected || gHovered);
            }
        }
        gDragging = null;
        gPanning = false;
        canvas.style.cursor = gHovered ? 'pointer' : 'grab';
    });

    canvas.addEventListener('mouseleave', () => {
        gDragging = null;
        gPanning = false;
        gHovered = null;
        canvas.style.cursor = 'grab';
    });

    canvas.addEventListener('wheel', e => {
        e.preventDefault();
        const rect = canvas.getBoundingClientRect();
        const mx = e.clientX - rect.left, my = e.clientY - rect.top;

        const oldZoom = gZoom;
        const delta = e.deltaY > 0 ? 0.9 : 1.1;
        gZoom = Math.max(0.2, Math.min(5, gZoom * delta));

        // Zoom toward cursor
        gPanX = mx - (mx - gPanX) * (gZoom / oldZoom);
        gPanY = my - (my - gPanY) * (gZoom / oldZoom);
    }, { passive: false });

    // Double-click to zoom in on node or reset
    canvas.addEventListener('dblclick', e => {
        const rect = canvas.getBoundingClientRect();
        const n = gFind(e.clientX - rect.left, e.clientY - rect.top);
        if (n) {
            if (n.type === 'tool') openToolSearch(n.label);
            else openRepo(n.label);
        } else {
            // Reset view
            gZoom = 1;
            gPanX = gW / 2;
            gPanY = gH / 2;
        }
    });

    window.addEventListener('resize', () => {
        if (currentView === 'graph') gResize();
    });
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
