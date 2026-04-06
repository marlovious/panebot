// ---------------------------------------------------------------------------
// PaneBot Chrome Extension — popup.js
// ---------------------------------------------------------------------------

const LOCAL_ADDR = 'ws://127.0.0.1:9090';

let currentUrl   = '';
let selectedPane = null;  // { address, pane }
let loadMode     = 'append-play';
let allHosts     = [];

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

document.addEventListener('DOMContentLoaded', async () => {
  // Check for URL passed from context menu
  const session = await chrome.storage.session.get('pendingUrl');
  if (session.pendingUrl) {
    currentUrl = session.pendingUrl;
    chrome.storage.session.remove('pendingUrl');
  } else {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    currentUrl = tab?.url || '';
  }
  document.getElementById('url-display').textContent = currentUrl;

  // Paste button
  document.getElementById('paste-btn').addEventListener('click', async () => {
    try {
      const text = await navigator.clipboard.readText();
      if (text.trim()) {
        currentUrl = text.trim();
        document.getElementById('url-display').textContent = currentUrl;
      }
    } catch {
      setStatus('Clipboard access denied', 'error');
    }
  });

  // Mode buttons
  document.querySelectorAll('.mode-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('.mode-btn').forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      loadMode = btn.dataset.mode;
    });
  });

  document.getElementById('send-btn').addEventListener('click', sendToPane);

  connectAndLoad();
});

// ---------------------------------------------------------------------------
// Single connection — get snapshot + state, done
// ---------------------------------------------------------------------------

function connectAndLoad() {
  const ws      = new WebSocket(LOCAL_ADDR);
  let   settled = false;

  const finish = () => {
    if (settled) return;
    settled = true;
    ws.close();
    renderPanes();
  };

  // Give the daemon 600ms to send everything then close
  const timer = setTimeout(finish, 600);

  ws.onmessage = (evt) => {
    let msg;
    try { msg = JSON.parse(evt.data); } catch { return; }

    if (msg.event === 'node:snapshot') {
      const localHost = {
        label:   msg.hostname || 'local',
        address: LOCAL_ADDR,
        panes:   (msg.panes || []).map(p => ({
          name:      p.name,
          pane_name: p.pane_name,
          online:    false,
          paused:    null,
          idle:      null,
        })),
      };
      allHosts = [localHost];

      // Queue remote host fetches
      const knownHosts = msg.known_hosts || [];
      if (knownHosts.length > 0) {
        Promise.all(knownHosts.map(h => fetchRemoteSnapshot(h.label, h.address)))
          .then(results => {
            results.forEach(r => { if (r) allHosts.push(r); });
            renderPanes();
          });
      }
    }

    if (msg.event === 'online') {
      applyOnline(msg);
      renderPanes();
    }
    if (msg.event === 'offline') {
      applyOffline(msg);
      renderPanes();
    }
  };

  ws.onopen  = () => {};
  ws.onerror = () => {
    clearTimeout(timer);
    setStatus('Daemon not found at ' + LOCAL_ADDR, 'error');
    document.getElementById('pane-list').innerHTML = '<div id="connecting">Daemon offline</div>';
  };
  ws.onclose = () => clearTimeout(timer);
}

// ---------------------------------------------------------------------------
// Remote snapshot — one connection, 500ms timeout
// ---------------------------------------------------------------------------

function fetchRemoteSnapshot(label, address) {
  return new Promise((resolve) => {
    const ws      = new WebSocket(address);
    const timeout = setTimeout(() => { ws.close(); resolve(null); }, 500);

    ws.onmessage = (evt) => {
      let msg;
      try { msg = JSON.parse(evt.data); } catch { return; }
      if (msg.event === 'node:snapshot') {
        clearTimeout(timeout);
        ws.close();
        resolve({
          label:   label || msg.hostname || address,
          address: address,
          panes:   (msg.panes || []).map(p => ({
            name:      p.name,
            pane_name: p.pane_name,
            online:    false,
            paused:    null,
            idle:      null,
          })),
        });
      }
    };
    ws.onerror = () => { clearTimeout(timeout); resolve(null); };
  });
}

// ---------------------------------------------------------------------------
// State helpers
// ---------------------------------------------------------------------------

function applyOnline(msg) {
  const host = allHosts.find(h => h.address === LOCAL_ADDR);
  if (!host) return;
  const pane = host.panes.find(p => p.name === msg.pane);
  if (!pane) return;
  pane.online = true;
  if (msg.state) {
    if (msg.state.paused      !== undefined) pane.paused = msg.state.paused;
    if (msg.state.idle_active !== undefined) pane.idle   = msg.state.idle_active;
  }
}

function applyOffline(msg) {
  const host = allHosts.find(h => h.address === LOCAL_ADDR);
  if (!host) return;
  const pane = host.panes.find(p => p.name === msg.pane);
  if (pane) pane.online = false;
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
    hostEl.className = 'host-label';
    hostEl.textContent = host.label + ' :: ' + host.address;
    list.appendChild(hostEl);

    host.panes.forEach(pane => {
      const row    = document.createElement('div');
      row.className = 'pane-row';
      if (selectedPane && selectedPane.address === host.address && selectedPane.pane === pane.name) {
        row.classList.add('selected');
      }

      const nameEl   = document.createElement('span');
      nameEl.className = 'pane-name';
      nameEl.textContent = pane.pane_name || pane.name;

      const statusEl = document.createElement('span');
      const { label, cls } = statusLabel(pane);
      statusEl.className = 'pane-status ' + cls;
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
    ws.send(JSON.stringify({
      command: 'loadfile',
      pane:    selectedPane.pane,
      args:    [currentUrl, loadMode],
    }));
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
