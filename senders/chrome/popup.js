// ---------------------------------------------------------------------------
// PaneBot Chrome Extension — popup.js
// ---------------------------------------------------------------------------

const LOCAL_ADDR = 'wss://127.0.0.1:9090';

let currentUrl   = '';
let selectedPane = null;
let loadMode     = 'append-play';
let allHosts     = [];
let sockets      = [];  // all open connections

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

document.addEventListener('DOMContentLoaded', async () => {
  const session = await chrome.storage.session.get('pendingUrl');
  if (session.pendingUrl) {
    currentUrl = session.pendingUrl;
    chrome.storage.session.remove('pendingUrl');
  } else {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    currentUrl = tab?.url || '';
  }
  document.getElementById('url-display').textContent = currentUrl;

  document.getElementById('paste-btn').addEventListener('click', async () => {
    try {
      const text = await navigator.clipboard.readText();
      if (text.trim()) {
        currentUrl = text.trim();
        document.getElementById('url-display').textContent = currentUrl;
      }
    } catch { setStatus('Clipboard access denied', 'error'); }
  });

  document.querySelectorAll('.mode-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('.mode-btn').forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      loadMode = btn.dataset.mode;
    });
  });

  document.getElementById('send-btn').addEventListener('click', sendToPane);

  connect(LOCAL_ADDR, 'local', true);
});

// ---------------------------------------------------------------------------
// Connect — opens a persistent WebSocket, stays open until popup closes
// ---------------------------------------------------------------------------

function connect(address, label, isLocal) {
  const ws = new WebSocket(address);
  sockets.push(ws);

  ws.onmessage = (evt) => {
    let msg;
    try { msg = JSON.parse(evt.data); } catch { return; }

    if (msg.event === 'node:snapshot') {
      const host = {
        label:     msg.hostname || label,
        address,
        collapsed: !isLocal,
        panes:     (msg.panes || []).map(p => ({
          name: p.name, pane_name: p.pane_name,
          online: false, paused: null, idle: null,
        })),
      };
      if (isLocal) {
        allHosts = [host];
        // Connect to remote nodes from known_hosts
        (msg.known_hosts || []).forEach(h => {
          if (!sockets.find(s => s.url === h.address)) connect(h.address, h.label, false);
        });
      } else {
        const idx = allHosts.findIndex(h => h.address === address);
        if (idx >= 0) allHosts[idx] = { ...allHosts[idx], ...host };
        else allHosts.push(host);
      }
      renderPanes();
    }

    if (msg.event === 'online' || msg.event === 'offline') {
      const host = allHosts.find(h => h.address === address);
      if (host) { applyState(host, msg, msg.event === 'online'); renderPanes(); }
    }

    if (msg.event === 'property-change') {
      const host = allHosts.find(h => h.address === address);
      if (!host) return;
      const pane = host.panes.find(p => p.name === msg.pane);
      if (!pane) return;
      if (msg.property === 'pause')       pane.paused = msg.value;
      if (msg.property === 'idle-active') pane.idle   = msg.value;
      renderPanes();
    }
  };

  ws.onerror = () => {
    sockets = sockets.filter(s => s !== ws);
    if (isLocal) {
      chrome.storage.local.get('certTrusted', (data) => {
        if (!data.certTrusted) {
          chrome.storage.local.set({ certTrusted: true });
          chrome.tabs.create({ url: LOCAL_ADDR.replace('wss://', 'https://') });
        }
      });
      setStatus('Daemon not found — trust cert in new tab', 'error');
      document.getElementById('pane-list').innerHTML = '<div id="connecting">Daemon offline</div>';
    } else {
      const idx = allHosts.findIndex(h => h.address === address);
      const entry = { label, address, failed: true, collapsed: false, panes: [] };
      if (idx >= 0) allHosts[idx] = entry; else allHosts.push(entry);
      renderPanes();
    }
  };

  ws.onclose = () => {
    sockets = sockets.filter(s => s !== ws);
    if (isLocal) {
      setStatus('Daemon disconnected — retrying...', 'error');
      setTimeout(() => {
        setStatus('');
        connect(LOCAL_ADDR, 'local', true);
      }, 2000);
    }
  };
}

// ---------------------------------------------------------------------------
// State helper
// ---------------------------------------------------------------------------

function applyState(host, msg, online) {
  const pane = host.panes.find(p => p.name === msg.pane);
  if (!pane) return;
  pane.online = online;
  if (online && msg.state) {
    if (msg.state.paused      !== undefined) pane.paused = msg.state.paused;
    if (msg.state.idle_active !== undefined) pane.idle   = msg.state.idle_active;
  }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

function renderPanes() {
  const list = document.getElementById('pane-list');
  list.innerHTML = '';

  if (allHosts.length === 0) {
    list.innerHTML = '<div id="connecting">No hosts found</div>';
    return;
  }

  allHosts.forEach(host => {
    const hostEl = document.createElement('div');
    hostEl.className    = 'host-label' + (host.failed ? ' failed' : '');
    hostEl.style.cursor = 'pointer';
    hostEl.textContent  = (host.collapsed ? '▶ ' : '▼ ') + host.label + ' :: ' + host.address;

    if (host.failed) {
      const trust = document.createElement('a');
      trust.textContent = ' ⚠ Trust';
      trust.href = '#';
      trust.style.cssText = 'color:#c08030;text-decoration:none;float:right;';
      trust.addEventListener('click', (e) => {
        e.preventDefault();
        chrome.tabs.create({ url: host.address.replace('wss://', 'https://') });
      });
      hostEl.appendChild(trust);
    }

    hostEl.addEventListener('click', () => { host.collapsed = !host.collapsed; renderPanes(); });
    list.appendChild(hostEl);

    if (host.failed || host.collapsed) return;

    host.panes.forEach(pane => {
      const row = document.createElement('div');
      row.className = 'pane-row';
      if (selectedPane?.address === host.address && selectedPane?.pane === pane.name) {
        row.classList.add('selected');
      }

      const nameEl = document.createElement('span');
      nameEl.className   = 'pane-name';
      nameEl.textContent = pane.pane_name || pane.name;

      const statusEl = document.createElement('span');
      const { label, cls } = statusLabel(pane);
      statusEl.className   = 'pane-status ' + cls;
      statusEl.textContent = label;

      row.appendChild(nameEl);
      row.appendChild(statusEl);
      row.addEventListener('click', () => {
        selectedPane = { address: host.address, pane: pane.name };
        document.getElementById('send-btn').disabled = false;
        document.querySelectorAll('.pane-row').forEach(r => r.classList.remove('selected'));
        row.classList.add('selected');
        setStatus('');
      });
      list.appendChild(row);
    });
  });

  document.getElementById('host-label').textContent =
    allHosts.length === 1 ? allHosts[0].label : allHosts.length + ' nodes';
}

function statusLabel(pane) {
  if (!pane.online) return { label: 'offline', cls: 'offline' };
  if (pane.idle)    return { label: 'stopped', cls: 'stopped' };
  if (pane.paused)  return { label: 'paused',  cls: 'paused'  };
  return                   { label: 'playing', cls: 'playing' };
}

// ---------------------------------------------------------------------------
// Send
// ---------------------------------------------------------------------------

function sendToPane() {
  if (!selectedPane || !currentUrl) return;
  const ws = new WebSocket(selectedPane.address);
  ws.onopen = () => {
    ws.send(JSON.stringify({ command: 'loadfile', pane: selectedPane.pane, args: [currentUrl, loadMode] }));
    setTimeout(() => ws.close(), 300);
    setStatus('Sent to ' + selectedPane.pane, 'ok');
  };
  ws.onerror = () => setStatus('Failed to connect to ' + selectedPane.address, 'error');
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

function setStatus(msg, cls) {
  const el = document.getElementById('status');
  el.textContent = msg;
  el.className   = cls || '';
}
