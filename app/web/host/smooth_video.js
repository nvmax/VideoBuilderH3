// The player for the scene clips.
//
// The Builder's own player is one <video> whose address is swapped at every cut. That empties the picture, restarts the
// decoder and shows the browser's control bar each time, and the clip starts a moment after the audio did.
// This one keeps two <video> elements stacked. The clip that comes next is loaded and held on its first frame while the
// current one plays, so a cut only changes which of the two is on top. The picture on screen is never emptied: a clip
// that was not ready in time is swapped in when its frame is there, and the old frame stays up until then.
//
// It answers to the names the Builder uses on a <video> (src, currentTime, play, pause, dataset, addEventListener ...),
// so the Builder's code is unchanged. The controls are drawn here, not by the browser, and fade away when the pointer rests.

const FIT = "position:absolute;left:0;top:0;width:100%;height:100%;object-fit:contain;background:#050505;";
const FORWARDED = ["error", "loadedmetadata", "pause", "play", "playing", "seeked", "ended"];
const SHOW_GIVE_UP_MS = 1500;
const CONTROLS_IDLE_MS = 1800;
const CLICK_DELAY_MS = 230;
const UNUSED_AHEAD_MS = 400;

function clock(seconds) {
  const value = Number.isFinite(seconds) && seconds > 0 ? seconds : 0;
  return `${Math.floor(value / 60)}:${String(Math.floor(value % 60)).padStart(2, "0")}`;
}

function makeSlot() {
  const video = document.createElement("video");
  video.playsInline = true;
  video.preload = "auto";
  video.controls = false;
  video.disablePictureInPicture = true;
  video.style.cssText = `${FIT}opacity:0;z-index:1;`;
  return { video, url: "", key: "", ahead: false };
}

function makeControlButton(label, title) {
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = label;
  button.title = title;
  button.style.cssText = "border:0;background:transparent;color:inherit;font-size:16px;line-height:1;cursor:pointer;padding:4px 6px;";
  return button;
}

/**
 * @param {{ toggle?: () => void, isPlaying?: () => boolean }} hooks `toggle` plays or pauses the Builder's timeline
 *   (what Space does), `isPlaying` says whether it is playing. Both are looked up when used, so they can be set up later.
 */
export function createSmoothVideo(hooks = {}) {
  const element = document.createElement("div");
  element.style.cssText = "display:none;position:relative;width:100%;height:100%;min-height:0;overflow:hidden;background:#050505;";
  const slots = [makeSlot(), makeSlot()];
  element.append(slots[0].video, slots[1].video);
  const bus = new EventTarget();
  const dataset = {};
  let shown = slots[0];
  let current = slots[0];
  let giveUpTimer = 0;
  // The clip the Builder last asked to have loaded ahead. `key` is the Builder's own name for a clip (path and cache bust).
  // The address cannot say whether it is the same clip: a scene without a cache bust gets a new time stamp in it each time.
  let primeRequest = null;

  const spare = (slot) => (slot === slots[0] ? slots[1] : slots[0]);
  const emit = (type) => bus.dispatchEvent(new Event(type));

  // Puts a slot on top. The old one is paused where it stands.
  function show(slot) {
    clearTimeout(giveUpTimer);
    if (slot === shown) return;
    const previous = shown;
    slot.video.style.zIndex = "2";
    slot.video.style.opacity = "1";
    previous.video.style.zIndex = "1";
    previous.video.style.opacity = "0";
    previous.video.pause();
    shown = slot;
    if (primeRequest && !primeRequest.slot) place(primeRequest);
  }

  // The slot the Builder works with is not on screen until it has a picture at the position it was asked for.
  function showWhenReady() {
    clearTimeout(giveUpTimer);
    if (current === shown) return;
    const video = current.video;
    if (video.readyState >= 2 && !video.seeking) {
      show(current);
      return;
    }
    giveUpTimer = setTimeout(() => show(current), SHOW_GIVE_UP_MS);
  }

  // The Builder sets a new address: use the slot that was loaded ahead when it is the right clip.
  function claim(url) {
    // The Builder sets the clip's key just before the address, see setPreviewVideoSource.
    const key = String(dataset.cacheKey || "");
    const target = current !== shown ? current : spare(current);
    const same = key ? target.key === key : target.url === url;
    const ready = same && target.video.readyState >= 2 && !target.video.error;
    target.ahead = false;
    if (!ready) {
      target.video.pause();
      target.video.src = url;
      target.url = url;
      target.key = key;
    }
    const previous = current;
    current = target;
    if (previous !== target) previous.video.pause();
    // A clip that was loaded ahead has its metadata already, so the event the Builder waits for would not come again.
    if (ready) queueMicrotask(() => { if (current === target) emit("loadedmetadata"); });
    showWhenReady();
  }

  // Load the next clip into the slot that is not on screen. While a swap is still waiting for its picture both slots are
  // busy, so the request waits until the swap is done.
  function prime(url) {
    primeRequest = { url, key: "", slot: null };
    place(primeRequest);
  }

  function place(request) {
    if (current !== shown) return;
    const target = spare(current);
    // The Builder asks for the clip after the next one just before it takes the next one, so a slot that holds a clip loaded
    // ahead and not used yet must not be replaced at once. The request waits for the clip to be taken (show() places it then),
    // and replaces it only when it stays unused, because the playhead went somewhere else.
    if (target.ahead && !request.unused) {
      if (!request.waiting) {
        request.waiting = true;
        setTimeout(() => {
          if (primeRequest !== request || request.slot) return;
          request.unused = true;
          place(request);
        }, UNUSED_AHEAD_MS);
      }
      return;
    }
    target.video.pause();
    target.video.src = request.url;
    target.url = request.url;
    target.key = request.key;
    target.ahead = true;
    request.slot = target;
  }

  for (const slot of slots) {
    for (const type of FORWARDED) slot.video.addEventListener(type, () => { if (slot === current) emit(type); });
    for (const type of ["loadeddata", "canplay", "seeked", "error"]) slot.video.addEventListener(type, () => { if (slot === current) showWhenReady(); });
  }

  // Stands in for the Builder's hidden "preload" video: setting its address loads the clip ahead.
  const standby = {
    // The Builder names the clip right after it sets the address, and asks for the name before it loads a clip a second time.
    dataset: {
      get cacheKey() { return primeRequest ? primeRequest.key : ""; },
      set cacheKey(value) {
        if (!primeRequest) return;
        primeRequest.key = String(value);
        if (primeRequest.slot) primeRequest.slot.key = primeRequest.key;
      },
    },
    get src() { return spare(current).url; },
    set src(url) { prime(String(url)); },
    load() {},
  };

  // A window the browser opened blank has full screen switched off, so the button and the key do nothing there.
  const canFullscreen = () => Boolean(element.ownerDocument.fullscreenEnabled);
  function toggleFullscreen() {
    if (!canFullscreen()) return;
    const doc = element.ownerDocument;
    if (doc.fullscreenElement) doc.exitFullscreen?.();
    else element.requestFullscreen?.().catch(() => {});
  }

  // ---- Controls: play and pause, the time in the clip and full screen. They show while the pointer moves.
  const bar = document.createElement("div");
  bar.style.cssText = "position:absolute;left:0;right:0;bottom:0;z-index:5;display:flex;align-items:center;gap:12px;padding:24px 14px 10px;background:linear-gradient(to top,rgba(0,0,0,.78),rgba(0,0,0,0));color:#e4e4e7;font:600 12px system-ui,sans-serif;opacity:0;pointer-events:none;transition:opacity .2s;";
  const track = document.createElement("div");
  track.style.cssText = "position:absolute;left:14px;right:14px;top:18px;height:3px;border-radius:2px;background:rgba(255,255,255,.25);";
  const fill = document.createElement("div");
  fill.style.cssText = "height:100%;width:0;border-radius:2px;background:#22d3ee;";
  track.append(fill);
  const playToggle = makeControlButton("▶", "Play or pause the timeline (Space)");
  const timeLabel = document.createElement("span");
  timeLabel.style.cssText = "font-variant-numeric:tabular-nums;";
  const grow = document.createElement("div");
  grow.style.cssText = "flex:1;";
  const fullToggle = makeControlButton("⛶", "Full screen (F or double-click)");
  bar.append(track, playToggle, timeLabel, grow, fullToggle);
  element.append(bar);
  playToggle.onclick = () => { playToggle.blur(); hooks.toggle?.(); };
  fullToggle.onclick = () => { fullToggle.blur(); toggleFullscreen(); };

  let idleTimer = 0;
  let frame = 0;
  let revealed = false;
  let idle = false;
  const setIdle = (value) => {
    idle = value;
    bus.dispatchEvent(new CustomEvent("idlechange", { detail: { idle: value } }));
  };
  function paint() {
    const video = current.video;
    const duration = video.duration;
    playToggle.textContent = hooks.isPlaying?.() ? "Ⅱ" : "▶";
    fullToggle.style.display = canFullscreen() ? "" : "none";
    timeLabel.textContent = `${clock(video.currentTime)} / ${clock(duration)}`;
    fill.style.width = Number.isFinite(duration) && duration > 0 ? `${Math.min(100, (video.currentTime / duration) * 100)}%` : "0%";
  }
  function follow() {
    if (!revealed) {
      frame = 0;
      return;
    }
    paint();
    frame = (element.ownerDocument.defaultView || window).requestAnimationFrame(follow);
  }
  function conceal() {
    if (bar.matches(":hover")) {
      idleTimer = setTimeout(conceal, CONTROLS_IDLE_MS);
      return;
    }
    revealed = false;
    bar.style.opacity = "0";
    bar.style.pointerEvents = "none";
    element.style.cursor = "none";
    setIdle(true);
  }
  function reveal() {
    clearTimeout(idleTimer);
    if (!revealed) {
      revealed = true;
      bar.style.opacity = "1";
      bar.style.pointerEvents = "auto";
      element.style.cursor = "";
      setIdle(false);
      if (!frame) follow();
    }
    idleTimer = setTimeout(conceal, CONTROLS_IDLE_MS);
  }
  element.addEventListener("pointermove", reveal);
  element.addEventListener("pointerdown", reveal);
  element.addEventListener("pointerleave", () => {
    clearTimeout(idleTimer);
    idleTimer = setTimeout(conceal, 400);
  });

  // A click on the picture plays or pauses, a double-click goes full screen.
  let clickTimer = 0;
  element.addEventListener("click", (event) => {
    if (bar.contains(event.target)) return;
    clearTimeout(clickTimer);
    clickTimer = setTimeout(() => hooks.toggle?.(), CLICK_DELAY_MS);
  });
  element.addEventListener("dblclick", (event) => {
    if (bar.contains(event.target)) return;
    clearTimeout(clickTimer);
    toggleFullscreen();
  });

  return {
    element,
    standby,
    style: element.style,
    dataset,
    get idle() { return idle; },

    // What the Builder reads from and writes to a <video>. All of it goes to the slot the Builder is working with.
    get src() { return current.url; },
    set src(url) { claim(String(url)); },
    get paused() { return current.video.paused; },
    get ended() { return current.video.ended; },
    get seeking() { return current.video.seeking; },
    get readyState() { return current.video.readyState; },
    get error() { return current.video.error; },
    get duration() { return current.video.duration; },
    get currentTime() { return current.video.currentTime; },
    set currentTime(value) { current.video.currentTime = value; },
    get muted() { return current.video.muted; },
    set muted(value) { for (const slot of slots) slot.video.muted = Boolean(value); },
    set controls(_value) {},
    set playsInline(_value) {},
    load() {},
    play() {
      const video = current.video;
      return !video.paused && !video.ended ? Promise.resolve() : video.play();
    },
    pause() { current.video.pause(); },
    removeAttribute(name) {
      if (name !== "src") return;
      const slot = current;
      slot.video.pause();
      slot.video.removeAttribute("src");
      slot.video.load();
      slot.url = "";
      if (slot !== shown) {
        current = shown;
        clearTimeout(giveUpTimer);
      }
    },
    addEventListener(type, listener, options) { bus.addEventListener(type, listener, options); },
    removeEventListener(type, listener, options) { bus.removeEventListener(type, listener, options); },

    /** The <video> that is on screen, for code that draws a frame from it. */
    frameSource() { return shown.video; },
    /** Call after the player was moved to another window: a picture that was waiting for its frame is checked again. */
    afterMove() { if (current !== shown) showWhenReady(); },
    /** Space plays and pauses, F goes full screen, in a window that holds only the player. Returns a function that stops it. */
    attachKeyboard(target) {
      const onKey = (event) => {
        if (event.ctrlKey || event.metaKey || event.altKey) return;
        const tag = String(event.target?.tagName || "").toLowerCase();
        if (tag === "input" || tag === "textarea" || tag === "select") return;
        if (event.code === "Space") {
          event.preventDefault();
          if (!event.repeat) hooks.toggle?.();
        } else if (event.key === "f" || event.key === "F") {
          toggleFullscreen();
        }
      };
      target.addEventListener("keydown", onKey);
      return () => target.removeEventListener("keydown", onKey);
    },
  };
}
