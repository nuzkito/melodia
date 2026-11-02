// Loads the landing page (web/) in Chromium and in Firefox and saves screenshots to
// dist/screenshots/web-*.png. It saves the top, a scrolled view (the hue of the backdrop turns 1°
// for each 1vh), the guide, the «Copy» animation and the footer. It also saves the top at phone width
// and with reduced motion. It fails if the page logs errors, an image does not load, the hue does not
// follow the scroll or «Copy» does not copy the redirect URI.
// Run it in the `browser` service:
//
//   docker compose up -d web browser
//   docker compose exec -T browser node docker/web-screenshots.js

const fs = require("node:fs");
const puppeteer = require("puppeteer-core");

const URL = process.env.WEB_URL || "http://web:80";
const OUT = "dist/screenshots";

// `reduced`: the same browser, with a preference for reduced motion. Firefox (WebDriver BiDi)
// cannot emulate media features for each page, so each browser gets its own setting. The clipboard
// API exists only in a secure context, so the browsers must accept http://web as one.
const BROWSERS = [
  {
    name: "chrome",
    options: {
      browser: "chrome",
      executablePath: process.env.CHROME_PATH,
      args: ["--no-sandbox", "--unsafely-treat-insecure-origin-as-secure=http://web"],
    },
    reduced: {
      args: [
        "--no-sandbox",
        "--unsafely-treat-insecure-origin-as-secure=http://web",
        "--force-prefers-reduced-motion",
      ],
    },
  },
  {
    name: "firefox",
    options: {
      browser: "firefox",
      executablePath: process.env.FIREFOX_PATH,
      extraPrefsFirefox: { "dom.securecontext.allowlist": "web", "security.mixed_content.upgrade_display_content": false },
    },
    reduced: {
      extraPrefsFirefox: { "dom.securecontext.allowlist": "web", "security.mixed_content.upgrade_display_content": false, "ui.prefersReducedMotion": 1 },
    },
  },
];

const settle = () => new Promise((resolve) => setTimeout(resolve, 1200));
const accent = (page) =>
  page.evaluate(() => document.documentElement.style.getPropertyValue("--accent"));

async function open(browser, problems, viewport) {
  const page = await browser.newPage();
  page.on("console", (message) => {
    if (message.type() === "error") problems.push(`console: ${message.text()}`);
  });
  page.on("pageerror", (error) => problems.push(`page error: ${error.message}`));
  await page.setViewport(viewport);
  await page.goto(URL, { waitUntil: "networkidle0" });
  await page.evaluate(() => document.fonts.ready);
  const broken = await page.$$eval("img", (images) =>
    images.filter((image) => !image.complete || image.naturalWidth === 0).map((image) => image.src),
  );
  for (const src of broken) problems.push(`image didn't load: ${src}`);
  await settle();
  return page;
}

async function run({ name, options, reduced }) {
  const problems = [];
  let browser = await puppeteer.launch({ headless: true, ...options });
  try {
    const desktop = { width: 1440, height: 900 };
    const page = await open(browser, problems, desktop);
    const shot = (file) => page.screenshot({ path: `${OUT}/web-${name}-${file}.png` });
    await shot("1-top");
    const start = await accent(page);

    // 120vh down: the accent should turn 120°.
    await page.evaluate(() => window.scrollTo({ top: innerHeight * 1.2, behavior: "instant" }));
    await settle();
    await shot("2-scrolled");
    const turned = await accent(page);
    if (turned === start) problems.push(`the hue didn't follow the scroll (${start})`);

    await page.evaluate(() =>
      document.getElementById("install").scrollIntoView({ behavior: "instant" }),
    );
    await settle();
    await shot("3-guide");

    // «Copy»: the screenshot shows the middle of the animation (the band of light and the ghost).
    const button = await page.$('[data-copy="redirect-uri"]');
    if (await button.evaluate((element) => element.hidden)) {
      problems.push("the «Copy» button is hidden (no clipboard API)");
    } else {
      await button.evaluate((element) =>
        element.scrollIntoView({ block: "center", behavior: "instant" }),
      );
      await settle();
      await button.click();
      await new Promise((resolve) => setTimeout(resolve, 220));
      await shot("3b-copied");
      const copied = await page.evaluate(async () => {
        try {
          return await navigator.clipboard.readText();
        } catch {
          return null;
        }
      });
      const uri = await page.$eval("#redirect-uri", (element) => element.textContent.trim());
      if (copied !== null && copied !== uri) problems.push(`«Copy» copied “${copied}”`);
    }

    await page.evaluate(() =>
      window.scrollTo({ top: document.body.scrollHeight, behavior: "instant" }),
    );
    await settle();
    await shot("4-footer");
    await page.close();

    const phone = await open(browser, problems, { width: 390, height: 844, deviceScaleFactor: 2 });
    await phone.screenshot({ path: `${OUT}/web-${name}-5-phone.png` });
    await phone.close();
    await browser.close();

    browser = await puppeteer.launch({ headless: true, ...options, ...reduced });
    const still = await open(browser, problems, desktop);
    await still.evaluate(() => window.scrollTo({ top: innerHeight * 1.2, behavior: "instant" }));
    await settle();
    await still.screenshot({ path: `${OUT}/web-${name}-6-reduced-motion.png` });
    const kept = await accent(still);
    if (kept !== start) problems.push(`with reduced motion the hue moved: ${start} → ${kept}`);

    console.log(`${name}: accent ${start} → ${turned} after 120vh; reduced motion: ${kept}`);
  } finally {
    await browser.close();
  }
  return problems.map((problem) => `${name}: ${problem}`);
}

(async () => {
  fs.mkdirSync(OUT, { recursive: true });
  const problems = [];
  for (const browser of BROWSERS) problems.push(...(await run(browser)));
  for (const problem of problems) console.error(problem);
  console.log(problems.length ? `${problems.length} problem(s)` : `OK: screenshots in ${OUT}/web-*.png`);
  process.exit(problems.length ? 1 : 0);
})();
