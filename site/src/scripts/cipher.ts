// Background: a field of ciphertext. Now and then a stretch of it decrypts
// into a word, holds, and encrypts again. Only changed cells are redrawn, at
// ~15 fps, and requestAnimationFrame stops while the tab is hidden.

const GLYPHS = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=";
const WORDS = [
  "hello", "привет", "ratchet", "safety", "olm", "nonce", "see you at 8",
  "ciphertext", "no accounts", "onion", "forward secrecy", "termchat",
];
const CIPHER = "rgba(88, 91, 112, 0.45)"; // Mocha surface2
const PLAIN = "rgba(203, 166, 247, 0.9)"; // Mocha mauve
const SIZE = 15;
const FRAME_MS = 66;
const FLIPS = 0.012; // share of cells that change each frame

const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
const random = (n: number) => Math.floor(Math.random() * n);
const glyph = () => GLYPHS[random(GLYPHS.length)];

type Word = { row: number; col: number; text: string; age: number };

const canvas = document.getElementById("cipher") as HTMLCanvasElement | null;
const ctx = canvas?.getContext("2d");
if (canvas && ctx) {
  let cols = 0;
  let rows = 0;
  let cw = 0;
  let grid: string[] = [];
  let words: Word[] = [];

  const cell = (i: number, text: string, colour: string) => {
    const x = (i % cols) * cw;
    const y = Math.floor(i / cols) * SIZE;
    ctx.clearRect(x, y, cw, SIZE);
    ctx.fillStyle = colour;
    ctx.fillText(text, x, y);
  };

  const resize = () => {
    const dpr = Math.min(devicePixelRatio || 1, 2);
    canvas.width = innerWidth * dpr;
    canvas.height = innerHeight * dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.font = `${SIZE - 2}px ui-monospace, Menlo, Consolas, monospace`;
    ctx.textBaseline = "top";
    cw = ctx.measureText("M").width + 2;
    cols = Math.ceil(innerWidth / cw);
    rows = Math.ceil(innerHeight / SIZE);
    grid = Array.from({ length: cols * rows }, glyph);
    words = [];
    ctx.clearRect(0, 0, innerWidth, innerHeight);
    grid.forEach((g, i) => cell(i, g, CIPHER));
  };

  // A word lives 45 frames: decrypts one letter per frame, holds, encrypts.
  const step = (w: Word) => {
    const n = [...w.text].length;
    [...w.text].forEach((ch, k) => {
      const i = w.row * cols + w.col + k;
      const shown = w.age > k && w.age < 30 + k;
      cell(i, shown ? ch : (grid[i] = glyph()), shown ? PLAIN : CIPHER);
    });
    w.age += 1;
    return w.age < 30 + n;
  };

  const frame = () => {
    for (let k = 0; k < grid.length * FLIPS; k++) {
      const i = random(grid.length);
      if (!words.some((w) => i >= w.row * cols + w.col && i < w.row * cols + w.col + w.text.length)) {
        cell(i, (grid[i] = glyph()), CIPHER);
      }
    }
    if (words.length < 3 && Math.random() < 0.06) {
      const text = WORDS[random(WORDS.length)];
      if (cols > text.length + 2) {
        words.push({ row: random(rows), col: random(cols - text.length), text, age: 0 });
      }
    }
    words = words.filter(step);
  };

  resize();
  let timer = 0;
  addEventListener("resize", () => {
    clearTimeout(timer);
    timer = window.setTimeout(resize, 150);
  });
  if (!still) {
    let last = 0;
    const loop = (now: number) => {
      if (now - last >= FRAME_MS) {
        last = now;
        frame();
      }
      requestAnimationFrame(loop);
    };
    requestAnimationFrame(loop);
  }
}

// The title arrives as ciphertext and resolves letter by letter.
for (const el of document.querySelectorAll<HTMLElement>("[data-decrypt]")) {
  const text = el.textContent ?? "";
  if (still) continue;
  let shown = 0;
  const tick = () => {
    el.textContent = [...text].map((ch, k) => (k < shown ? ch : glyph())).join("");
    shown += 0.34;
    if (shown <= text.length) setTimeout(tick, 45);
    else el.textContent = text;
  };
  tick();
}
