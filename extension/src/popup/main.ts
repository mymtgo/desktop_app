// Popup page: wires popup.ts to the document and the background.
import { startPopup, type PopupElement } from './popup'

startPopup({
  element: (id) => document.getElementById(id) as unknown as PopupElement,
  createItem: () => document.createElement('li') as unknown as PopupElement,
  send: (message) => chrome.runtime.sendMessage(message),
  isFirefox: navigator.userAgent.includes('Firefox'),
})
