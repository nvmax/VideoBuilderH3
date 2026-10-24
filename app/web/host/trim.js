// Hides the parts of the pack's Builder that this app does not use, without changing the copied files. The Builder builds
// its screens from script, so a watcher hides each piece when it appears.

// Buttons by their label with the spaces taken out.
const HIDE_BUTTON_LABELS = new Set([
  "WizardLegacy", "WizardBeta", "UpdatetoLatest", "BuildFullVideo", "BuildFullFLFVideo", "What'sNew", "LTX",
  "PromptCreator(Legacy)", "SendToPromptCreator", "ImportDataFromPromptCreator", "ConvertLTXVideoPromptstoMiniMaxH3",
]);
// Labels that start the same way. A button in a Tools pane row hides with its row.
const HIDE_CARD_LABEL_STARTS = ["FaceFix"];
// Buttons by their accessible name or hint.
const HIDE_BY_ATTRIBUTE = [
  'button[aria-label="Close Video Builder"]', // the window closes the app
  'button[aria-label="Refresh Builder UI"]', // the app has no page to reload
  'button[title="What does the updater do?"]',
  'a[href*="buymeacoffee.com"]',
];
// A status line the Builder shows at the top, found by its first words.
const HIDE_BANNER_TEXT = /^AI Video Builder\b/;

// "important" keeps the Builder from showing the element again when it redraws a menu.
function hide(element) {
  if (element && element.style.getPropertyValue("display") !== "none") element.style.setProperty("display", "none", "important");
}

// The element itself when it matches, then everything inside it that does.
function matching(root, selector) {
  return [...(root.matches?.(selector) ? [root] : []), ...root.querySelectorAll(selector)];
}

function trim(root) {
  for (const selector of HIDE_BY_ATTRIBUTE) {
    for (const found of matching(root, selector)) hide(selector.startsWith("a[") ? found.closest("div") || found : found);
  }
  for (const button of matching(root, "button")) {
    const label = (button.textContent || "").replace(/\s+/g, "");
    if (HIDE_BUTTON_LABELS.has(label) || HIDE_CARD_LABEL_STARTS.some((start) => label.startsWith(start))) {
      hide(button.closest(".vrgdg-builder-tool-row") || button);
    }
  }
  for (const span of matching(root, "span")) {
    if (HIDE_BANNER_TEXT.test((span.textContent || "").trim()) && span.parentElement?.querySelector("button")) hide(span.parentElement);
  }
}

export function startTrimming() {
  trim(document);
  new MutationObserver((records) => {
    for (const record of records) {
      for (const node of record.addedNodes) {
        if (node.nodeType === 1) trim(node);
      }
    }
  }).observe(document.documentElement, { childList: true, subtree: true });
}
