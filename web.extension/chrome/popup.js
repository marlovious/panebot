// ---------------------------------------------------------------------------
// PaneBot Chrome Extension — popup.js
//
// Flow:
//   1. Get current tab URL
//   2. Connect to local daemon (ws://127.0.0.1:9090)
//   3. Receive node:snapshot — get local panes + known_hosts
//   4. Connect to each known_host, get their panes too
//   5. Render pane list grouped by host
//   6. User picks pane + mode, clicks Send
//   7. Open WS to that host, send loadfile, close
// ---------------------------------------------------------------------------

const LOCAL_ADDR = 'ws://127.0.0.1:9090';

let currentUrl    = '';
let selectedPane  = null;  // { address, pane }
let loadMode      = 'append-play';
let allHosts      = [];    // [{ label, address, panes: [] }]

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

document.addEventListener('DOMContentLoaded', async () => {
  // Check for a URL passed from the context menu right-click
  const session = await chrome.storage.session.get('pendingUrl');
  if (session.pendingUrl) {
    currentUrl = session.pendingUrl;
    chrome.storage.session.remove('pendingUrl');
  } else {
    // Fall back to current tab URL
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    currentUrl = tab?.url || '';
  }
  document.getElementById('url-display').textContent = currentUrl;

  // Mode buttons
  document.querySelectorAll('.mode-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('.mode-btn').forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      loadMode = btn.dataset.mode;
    });
  });

  // Send button
  document.getElementById('send-btn').addEventListener('click', sendToPane);

  // Connect to local daemon
  connectToLocal();
});

// ---------------------------------------------------------------------------
// Connect to local daemon, get snapshot + known_hosts
// ---------------------------------------------------------------------------

function connectToLocal() {
  const ws = new WebSocket(LOCAL_ADDR);

  ws.onopen = () => {};

  ws.onmessage = (evt) => {
    let msg;
    try { msg = JSON.parse(evt.data); } catch { return; }

    if (msg.event === 'node:snapshot') {
      ws.close();

      // Local host
      const localHost = {
        label:   msg.hostname || 'local',
        address: LOCAL_ADDR,
        panes:   (msg.panes || []).map(p => ({
          name:      p.name,
          pane_name: p.pane_name,
          pane_type: p.pane_type,
          online:    false,
          paused:    null,
          idle:      null,
        })),
      };

      allHosts = [localHost];

      // Render local panes immediately — don't wait for remotes
      fetchLiveState(localHost, LOCAL_ADDR).then(() => renderPanes());
      renderPanes();

      // Fetch remote hosts progressively — each renders as it arrives
      const knownHosts = msg.known_hosts || [];
      knownHosts.forEach(h => {
        fetchRemoteSnapshot(h.label, h.address).then(result => {
          if (result) {
            allHosts.push(result);
            renderPanes();
          }
        });
      });
    }

    // Apply live state updates to local host
    if (msg.event === 'online' || msg.event === 'offline' || msg.event === 'property-change') {
      applyStateEvent(allHosts[0], msg);
      renderPanes();
    }
  };

  ws.onerror = () => {
    setStatus('Cannot connect to local daemon', 'error');
    document.getElementById('connecting').textContent = 'Daemon not found at ' + LOCAL_ADDR;
  };
}

// ---------------------------------------------------------------------------
// Fetch snapshot from a remote host
// ---------------------------------------------------------------------------

function fetchRemoteSnapshot(label, address) {
  return new Promise((resolve) => {
    const ws = new WebSocket(address);
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
            pane_type: p.pane_type,
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
// Fetch live pane state (online/offline/properties) for a host
// ---------------------------------------------------------------------------

function fetchLiveState(host, address) {
  return new Promise((resolve) => {
    const ws = new WebSocket(address);
    const timeout = setTimeout(() => { ws.close(); resolve(); }, 500);
    let gotSnapshot = false;

    ws.onmessage = (evt) => {
      let msg;
      try { msg = JSON.parse(evt.data); } catch { return; }

      if (msg.event === 'node:snapshot') { gotSnapshot = true; return; }

      if (gotSnapshot) {
        applyStateEvent(host, msg);
        // Once we've got a few state events, close
        if (msg.event === 'online' || msg.event === 'offline') {
          clearTimeout(timeout);
          setTimeout(() => { ws.close(); resolve(); }, 300);
        }
      }
    };

    ws.onerror = () => { clearTimeout(timeout); resolve(); };
  });
}

// ---------------------------------------------------------------------------
// Apply state events to a host's pane list
// ---------------------------------------------------------------------------

function applyStateEvent(host, msg) {
  if (!host) return;
  const pane = host.panes.find(p => p.name === msg.pane);
  if (!pane) return;

  if (msg.event === 'online') {
    pane.online = true;
    if (msg.state) {
      if (msg.state.paused      !== undefined) pane.paused = msg.state.paused;
      if (msg.state.idle_active !== undefined) pane.idle   = msg.state.idle_active;
    }
  }
  if (msg.event === 'offline') {
    pane.online = false;
  }
  if (msg.event === 'property-change') {
    if (msg.property === 'pause')       pane.paused = msg.value;
    if (msg.property === 'idle-active') pane.idle   = msg.value;
  }
}

// ---------------------------------------------------------------------------
// Render pane list
// ---------------------------------------------------------------------------

function renderPanes() {
  const list = document.getElementById('pane-list');
  list.innerHTML = '';

  if (allHosts.length === 0) {
    list.innerHTML = '<div id="connecting">No hosts found</div>';
    return;
  }

  allHosts.forEach(host => {
    // Host label
    const hostEl = document.createElement('div');
    hostEl.className = 'host-label';
    hostEl.textContent = host.label + ' :: ' + host.address;
    list.appendChild(hostEl);

    host.panes.forEach(pane => {
      const row = document.createElement('div');
      row.className = 'pane-row';

      const isSelected = selectedPane &&
        selectedPane.address === host.address &&
        selectedPane.pane === pane.name;

      if (isSelected) row.classList.add('selected');

      const nameEl = document.createElement('span');
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
  if (!pane.online)              return { label: 'offline', cls: 'offline' };
  if (pane.idle)                 return { label: 'stopped', cls: 'stopped' };
  if (pane.paused)               return { label: 'paused',  cls: 'paused'  };
  return                                { label: 'playing', cls: 'playing' };
}

// ---------------------------------------------------------------------------
// Send loadfile to selected pane
// ---------------------------------------------------------------------------

function sendToPane() {
  if (!selectedPane || !currentUrl) return;

  const ws = new WebSocket(selectedPane.address);

  ws.onopen = () => {
    const cmd = JSON.stringify({
      command: 'loadfile',
      pane:    selectedPane.pane,
      args:    [currentUrl, loadMode],
    });
    ws.send(cmd);
    setTimeout(() => ws.close(), 300);
    setStatus('Sent to ' + selectedPane.pane, 'ok');
  };

  ws.onerror = () => {
    setStatus('Failed to connect to ' + selectedPane.address, 'error');
  };
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

function setStatus(msg, cls) {
  const el = document.getElementById('status');
  el.textContent  = msg;
  el.className    = cls || '';
}
