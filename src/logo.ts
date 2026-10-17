import type { Logo } from './types';

export type LogoMode = 'logo' | 'photo';

export function loadImage(file: File): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(file);
    const img = new Image();
    img.onload = () => {
      URL.revokeObjectURL(url);
      resolve(img);
    };
    img.onerror = () => {
      URL.revokeObjectURL(url);
      reject(new Error('not an image'));
    };
    img.src = url;
  });
}

function b64(bytes: Uint8Array): string {
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

// A thermal head only does black or nothing. Logos and text get a clean
// threshold; photos get Floyd-Steinberg error diffusion.
export function rasterize(img: HTMLImageElement, widthDots: number, mode: LogoMode, light: number): Logo {
  const w = Math.max(8, Math.min(576, Math.round(widthDots)));
  const h = Math.max(1, Math.min(800, Math.round((img.naturalHeight * w) / img.naturalWidth)));
  const c = document.createElement('canvas');
  c.width = w;
  c.height = h;
  const g = c.getContext('2d', { willReadFrequently: true })!;
  g.fillStyle = '#fff';
  g.fillRect(0, 0, w, h);
  g.drawImage(img, 0, 0, w, h);
  const px = g.getImageData(0, 0, w, h).data;

  const lum = new Float32Array(w * h);
  for (let i = 0; i < w * h; i++) {
    let v = 0.299 * px[i * 4] + 0.587 * px[i * 4 + 1] + 0.114 * px[i * 4 + 2];
    // dots bleed on thermal paper, so photos come out darker than on screen
    if (mode === 'photo') v = 255 * Math.pow(v / 255, 0.8);
    lum[i] = v + light;
  }

  const row = Math.ceil(w / 8);
  const bits = new Uint8Array(row * h);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = y * w + x;
      const black = lum[i] < 128;
      if (black) bits[y * row + (x >> 3)] |= 0x80 >> (x & 7);
      if (mode !== 'photo') continue;
      const err = lum[i] - (black ? 0 : 255);
      if (x + 1 < w) lum[i + 1] += (err * 7) / 16;
      if (y + 1 < h) {
        if (x > 0) lum[i + w - 1] += (err * 3) / 16;
        lum[i + w] += (err * 5) / 16;
        if (x + 1 < w) lum[i + w + 1] += err / 16;
      }
    }
  }

  const out = g.createImageData(w, h);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const j = (y * w + x) * 4;
      const on = bits[y * row + (x >> 3)] & (0x80 >> (x & 7));
      out.data[j] = out.data[j + 1] = out.data[j + 2] = 28;
      out.data[j + 3] = on ? 255 : 0;
    }
  }
  g.clearRect(0, 0, w, h);
  g.putImageData(out, 0, 0);
  return { w, h, bits: b64(bits), png: c.toDataURL('image/png') };
}
