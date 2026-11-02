// Runs in each frame of the player page, before all other scripts. The Spotify SDK runs in an
// iframe. It pauses and resumes there when a different device asks (Spotify Connect), and the page
// cannot stop that. Thus the media elements fade themselves. The page's own pause and resume
// already fade (they first set the volume to 0), and then these functions do nothing.
(() => {
  if (window.top === window || !window.HTMLMediaElement) {
    return;
  }
  const FADE_MS = 150;
  const STEP_MS = 20;
  // A track that is about to end by itself does not fade.
  const END_MARGIN_S = 0.5;
  const proto = HTMLMediaElement.prototype;
  const nativePause = proto.pause;
  const nativePlay = proto.play;
  const nativeLoad = proto.load;
  // The fade in progress of each element: `{ timer, volume, finish }`.
  const fades = new WeakMap();

  const stopFade = (element, pause) => {
    const fade = fades.get(element);
    if (!fade) {
      return;
    }
    clearInterval(fade.timer);
    fades.delete(element);
    if (pause) {
      nativePause.call(element);
    }
    element.volume = fade.volume;
  };

  proto.pause = function () {
    const element = this;
    const silent = element.paused || element.ended || element.muted || element.volume === 0;
    const ending = element.duration - element.currentTime < END_MARGIN_S;
    if (silent || ending || fades.has(element)) {
      return nativePause.call(element);
    }
    const volume = element.volume;
    const steps = Math.max(1, Math.round(FADE_MS / STEP_MS));
    let step = 0;
    const timer = setInterval(() => {
      step += 1;
      if (step < steps) {
        element.volume = volume * (1 - step / steps);
      } else {
        stopFade(element, true);
      }
    }, STEP_MS);
    fades.set(element, { timer, volume });
  };

  // A «play» during the fade-out cancels the pause: the music continues to play.
  proto.play = function () {
    stopFade(this, false);
    return nativePlay.call(this);
  };

  // A new track loads: no fade, the old track stops immediately.
  proto.load = function () {
    stopFade(this, true);
    return nativeLoad.call(this);
  };
})();
