// ---------------------------------------------------------------------------
// PaneBot background service worker
//
// Registers a context menu item on links.
// When clicked, stores the link URL so the popup can pick it up.
// ---------------------------------------------------------------------------

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id:       'panebot-send',
    title:    'Send to PaneBot',
    contexts: ['link', 'video', 'audio'],
  });
});

chrome.contextMenus.onClicked.addListener((info) => {
  if (info.menuItemId !== 'panebot-send') return;

  const url = info.linkUrl || info.srcUrl || '';
  if (!url) return;

  // Store the URL so the popup reads it on open
  chrome.storage.session.set({ pendingUrl: url });

  // Open the popup
  chrome.action.openPopup();
});
