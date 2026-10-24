// Runs `step` once per display frame while `isActive()` holds.
//
// The Builder follows the audio with the audio element's `timeupdate` event, which comes about four times a second. A
// scene cut was seen up to 250 ms late and the playhead moved in steps. This runs the same update every frame.
// `ensure()` is cheap and can be called from any slow tick (the `timeupdate` itself): it starts the loop and restarts one
// that stopped, for example because the window it was scheduled on was closed or hidden. `windowOf` says which window to
// schedule on, so the loop follows the player into a pop-out window and keeps running when the Builder's window is covered.

export function createFramePump({ isActive, step, windowOf = () => window }) {
  let handle = 0;
  let owner = null;
  let lastRun = 0;

  function schedule() {
    owner = windowOf() || window;
    handle = owner.requestAnimationFrame(run);
  }

  function run() {
    handle = 0;
    if (!isActive()) return;
    lastRun = performance.now();
    step();
    schedule();
  }

  return {
    ensure() {
      if (!isActive()) return;
      if (handle && performance.now() - lastRun < 300) return;
      if (handle) {
        try {
          owner.cancelAnimationFrame(handle);
        } catch {
          // The window is gone, so there is nothing to cancel.
        }
      }
      lastRun = performance.now();
      schedule();
    },
  };
}
