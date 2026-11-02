// Melodia's landing page. The backdrop (two glows over BG, as in backdrop.rs of the app) turns
// its hue as the page scrolls: 1° for each 1vh, from Spotify green. The complementary color is
// always half a turn away. The hue eases towards the scroll, so a jump of the wheel shows no
// steps. With reduced motion, it stays at the start. The gradient of the title turns with it. Also:
// the «Copy» button and the version on the download button.

const BG = [11, 11, 14];
const START = [30, 215, 96];
/// The light that each glow adds at its center, as in backdrop.rs (`light`).
const ACCENT_OPACITY = 0.2;
const COMPLEMENT_OPACITY = 0.14;
const DEGREES_PER_VH = 1;
/// The time constant of the easing, in seconds.
const EASING = 0.15;

const toLinear = (c) => {
  const s = c / 255;
  return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
};
const toGamma = (l) => {
  const s = l <= 0.0031308 ? l * 12.92 : 1.055 * l ** (1 / 2.4) - 0.055;
  return Math.min(255, Math.max(0, s * 255));
};

// egui turns the hue (`HsvaGamma`) in HSV over linear light, not over sRGB. The page does the
// same, so the complementary color of the green is the purple of the app (215, 30, 200), not the
// sRGB purple.
function toHsv(rgb) {
  const [r, g, b] = rgb.map(toLinear);
  const max = Math.max(r, g, b);
  const d = max - Math.min(r, g, b);
  let h = 0;
  if (d > 0) {
    if (max === r) h = ((g - b) / d + 6) % 6;
    else if (max === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
  }
  return { h: h * 60, s: max === 0 ? 0 : d / max, v: max };
}

function fromHsv({ h, s, v }) {
  const f = (n) => {
    const k = (n + h / 60) % 6;
    return v - v * s * Math.max(0, Math.min(k, 4 - k, 1));
  };
  return [f(5), f(3), f(1)].map(toGamma);
}

const start = toHsv(START);

/// The light that changes BG into `color` laid over it with `opacity`.
const light = (color, opacity) =>
  color.map((c, i) => Math.max(0, (c - BG[i]) * opacity).toFixed(2)).join(" ");

const root = document.documentElement.style;

/// Tints the page `degrees` around the wheel from the start.
function paint(degrees) {
  const hue = (((start.h + degrees) % 360) + 360) % 360;
  const accent = fromHsv({ ...start, h: hue });
  const complement = fromHsv({ ...start, h: (hue + 180) % 360 });
  root.setProperty("--accent", accent.map((c) => c.toFixed(1)).join(" "));
  const quarter = fromHsv({ ...start, h: (hue + 90) % 360 });
  root.setProperty("--quarter", quarter.map((c) => c.toFixed(1)).join(" "));
  root.setProperty("--complement", complement.map((c) => c.toFixed(1)).join(" "));
  root.setProperty("--light-accent", light(accent, ACCENT_OPACITY));
  root.setProperty("--light-complement", light(complement, COMPLEMENT_OPACITY));
}

// The turn of the scroll, in degrees, without wrap. Thus the easing never goes the long way round
// when it crosses 360°.
let target = 0;
let shown = 0;
let frame = 0;
let last = 0;

function scrolled() {
  return (window.scrollY / (window.innerHeight / 100)) * DEGREES_PER_VH;
}

function tick(now) {
  const dt = last ? (now - last) / 1000 : 0;
  last = now;
  shown += (target - shown) * (1 - Math.exp(-dt / EASING));
  if (Math.abs(target - shown) < 0.01) shown = target;
  paint(shown);
  // It stops to ask for frames when it gets to the target, so the page is idle.
  frame = shown === target ? 0 : requestAnimationFrame(tick);
  if (!frame) last = 0;
}

function follow() {
  target = scrolled();
  if (!frame) frame = requestAnimationFrame(tick);
}

const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");

function setUp() {
  cancelAnimationFrame(frame);
  frame = 0;
  last = 0;
  window.removeEventListener("scroll", follow);
  window.removeEventListener("resize", follow);
  if (reduced.matches) {
    target = shown = 0;
    paint(0);
    return;
  }
  // It starts at the current position of the page (a reload keeps the scroll).
  target = shown = scrolled();
  paint(shown);
  window.addEventListener("scroll", follow, { passive: true });
  window.addEventListener("resize", follow, { passive: true });
}

reduced.addEventListener("change", setUp);
setUp();

// «Copy» buttons: they show only where the browser lets the page write to the clipboard. They say
// “Copied” for a moment, as the ＋ of the app changes into ✓. A band of light crosses the address,
// and a ghost of the address rises out of the field (not with reduced motion).
function restart(element, name) {
  element.classList.remove(name);
  void element.offsetWidth; // A new layout lets the animation start again from the beginning.
  element.classList.add(name);
}

function lift(source) {
  // A clone gets the same font and wraps in the same way.
  const ghost = source.cloneNode(true);
  ghost.removeAttribute("id");
  ghost.className = "copy-ghost";
  ghost.setAttribute("aria-hidden", "true");
  ghost.style.left = `${source.offsetLeft}px`;
  ghost.style.top = `${source.offsetTop}px`;
  // offsetWidth rounds down: a narrower ghost puts the last letter on a new line.
  ghost.style.width = `${Math.ceil(source.getBoundingClientRect().width)}px`;
  ghost.addEventListener("animationend", () => ghost.remove());
  source.parentElement.append(ghost);
}

if (navigator.clipboard) {
  for (const button of document.querySelectorAll("[data-copy]")) {
    const source = document.getElementById(button.dataset.copy);
    let timer = 0;
    button.hidden = false;
    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(source.textContent.trim());
      } catch {
        return;
      }
      button.textContent = "Copied";
      restart(button, "copied");
      restart(source, "copied");
      if (!reduced.matches) lift(source);
      clearTimeout(timer);
      timer = setTimeout(() => {
        button.textContent = "Copy";
        button.classList.remove("copied");
        source.classList.remove("copied");
      }, 1400);
    });
  }
}

// The download button shows the published version (version.json, the manifest that the
// installed apps check). Without it, the button keeps its text.
fetch("version.json", { cache: "no-store" })
  .then((response) => (response.ok ? response.json() : Promise.reject()))
  .then(({ version }) => {
    if (typeof version !== "string" || !version) return;
    const label = document.getElementById("download-version");
    label.textContent = version;
    label.hidden = false;
  })
  .catch(() => {});
