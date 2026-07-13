# LumiBase Design System

> **Design tools from the future.**
> Brand & UI system for **LumiBase** — a suite of AI-powered design tools for Figma.

LumiBase makes AI design utilities that live inside Figma. The brand is built around four products, each represented by a glossy planet in a cosmic "solar system":

- **Magician** — "Cast a spell on your Figma designs." AI design utilities (Magic Icons, Magic Image, Magic Copy, Magic Rename). Accent: **violet**.
- **Genius** — "Your AI design companion in Figma." In-line suggestions that work with your design system. Accent: **blue**.
- **Automator** — "Automate design tasks in Figma with a single click." Drag-and-drop, no-code automations. Accent: **green**.
- **UI-AI** — "User interface AI models made by designers, for designers." (Coming soon.) Accent: **blue**.

The whole brand world is dark, spacey and premium: a near-black gradient, a faint starfield, orbiting glossy planets, glass pill controls and softly glowing accents.

## Sources

- **Figma:** `LumiBase.fig` (attached) — a single-page recreation of **lumibase.dev** (1440 × 8148). All tokens, copy and imagery in this system were extracted from it. The page tree lives under `/Page-1/lumibase.dev/…`.
- Public reference: the historical lumibase.dev marketing site (LumiBase was acquired by Figma in 2023). The attached file is the source of truth — not the live site.

---

## CONTENT FUNDAMENTALS

**Voice — confident, playful, plain-spoken.** LumiBase talks like a clever design tool that wants to delight you. Big claims delivered simply.

- **Casing:** Sentence case everywhere — headlines, buttons, nav. Product names are the only proper nouns (Magician, Genius, Automator, UI-AI). Badges/labels are occasionally ALL-CAPS micro text (e.g. `PRODUCTIVITY`).
- **Person:** Second person — "your designs", "your workflow", "your entire team". The products are characters ("Figma's new AI bestie", "Your AI spellbook").
- **Headlines** are short and declarative, often a metaphor: *"Cast a spell on your Figma designs."*, *"Wave goodbye to Lorem Ipsum"*, *"Design tools from the future."*
- **Subheads / body** explain the benefit plainly in one sentence: *"Unlock your creativity and bring ideas to life with AI-powered design utilities."*
- **Magic/cosmic motif** runs through Magician copy (spell, conjure, spellbook, magically). Other products stay grounded and practical.
- **Punctuation:** headlines usually end with a period. No exclamation marks. No emoji in product copy.
- **CTAs** are often the bare product domain — `magician.design`, `genius.design`, `automator.design`, `ui-ai.com` — rendered as a glass pill with a small status dot. Generic CTAs: "Explore the future", "Join the future", "Generate", "Download".

Example specimens:
- Hero: **"Design tools from the future."** / "Unleash your creativity with LumiBase's AI-powered design tools."
- Section open: **"Genius"** / "Your AI design companion in Figma."
- Card: **"Build powerful automations without code"** / "Build any automation with drag-and-drop ease."

---

## VISUAL FOUNDATIONS

**Overall vibe:** dark, cosmic, premium, glassy. Content floats over a deep-space backdrop.

- **Backgrounds:** a fixed diagonal gradient `linear-gradient(135deg, #1E1E20 0%, #0E0E11 32%)` (`--gradient-page`) with a faint, repeating **starfield** of tiny white dots. Imagery is composited *over* this — no flat panels behind the whole page.
- **Color:** mostly monochrome dark surfaces (`#1D1C20` → `#34323F`) with text in white → `#BDBDC0` → `#A9A9A9`. Color appears sparingly as **brand accents** — violet `#7B61FF` (primary), blue `#18A0FB`, green `#2EC47C`, red `#E85656` — and as the glossy planet renders.
- **Glass system:** controls (nav, buttons, chips) are translucent white `rgba(255,255,255,0.08)` with a 1px **inset hairline ring** `inset 0 0 0 1px rgba(255,255,255,0.08)` (`--ring-glass`) instead of a solid border. Hover lifts the fill to ~14% white. This is *the* signature treatment.
- **Cards:** dark solid surface (`--color-surface-1` #1D1C20), large **24px** radius, the same glass hairline ring, no heavy drop shadow on the page (depth comes from the ring + the dark-on-dark layering). Feature cards put a visual/illustration area on top or bottom and copy in the other half.
- **Corner radii:** pills **32px**, cards **24px**, controls **12–16px**, chips **8px**. Everything is generously rounded; nothing is sharp.
- **Glow:** brand color is expressed as soft outer glow behind planets and primary CTAs — `0 0 80px rgba(123,97,255,0.45)` (`--glow-violet`) / blue equivalent. Glossy spheres carry a white radial sheen top-left and a dark inner shadow bottom.
- **Shadows:** restrained. `--shadow-sm` on small lifted dots/handles, `--shadow-md/lg` for floating UI in product mockups. The page itself relies on the hairline ring, not shadows.
- **Type:** **Inter**, weights 400/500/600/700. Hero display is 75px/86px bold with **−0.2px** tracking; product titles ~48px bold; card headings 19px semibold; body 15px medium in secondary grey; labels 13px semibold.
- **Spacing:** 8px base grid; very generous vertical rhythm between sections (~120px) on the marketing page. Content max-width 1200px, page 1440px, nav 72px tall.
- **Motion:** smooth and calm. Planets **orbit** on slow linear rotations (26s–96s). Interactive transitions use `--ease-out` (cubic-bezier(0.22,1,0.36,1)) at ~240ms. No bounce, no aggressive easing.
- **Hover states:** glass fills brighten (8% → 14% white); solid/violet buttons brighten ~8%. **Press:** subtle, no large scale changes.
- **Transparency & blur:** `backdrop-filter: blur(12px)` on the floating pill nav so the starfield shows through. Glass surfaces are semi-transparent; cards are opaque.
- **Imagery color vibe:** cool, jewel-toned, glossy 3D spheres on black — high contrast, slightly saturated, with strong specular highlights. No photography; no hand-drawn illustration.

---

## ICONOGRAPHY

- **Logo:** the LumiBase "D" — a half-disc mark (`assets/logo-mark.svg`) plus a wordmark (`assets/logo-wordmark.svg`), both white on dark. The hero hero-marks the D as a glossy white sphere with a cut-out D (`assets/d-cutout.png`).
- **Brand imagery is 3D, not iconographic:** the dominant "icons" are the **glossy planet PNGs** (`planet-magician`, `planet-genius`, `planet-blue`, `planet-green`, `planet-red`) used as product avatars and orbiting bodies. Copy these in; **never** redraw them.
- **UI icons:** the source page uses small line/glyph vectors for toolbar and feature chips. There is no bundled icon font. For new UI work, use a **thin-stroke line set** — [Lucide](https://lucide.dev) (CDN) is the closest match to the page's 1.5px line icons. **(Substitution — flag to the user if exact icons matter.)**
- **Glyphs as decoration:** small unicode/SVG glyphs (✦ ◆ ★ ❖) appear inside the Magician "icon generator" tile and UI-AI model tags. Used decoratively, in muted grey with the active one in violet.
- **Emoji:** not used in product copy or UI.

---

## INDEX / MANIFEST

**Root**
- `styles.css` — global entry (only `@import`s the token files). Consumers link this.
- `readme.md` — this guide.
- `SKILL.md` — Agent-Skills front-matter for use in Claude Code.

**Tokens** (`tokens/`)
- `colors.css` — backgrounds, surfaces, glass, text, brand accents, borders + semantic aliases.
- `typography.css` — Inter stack, weights, type scale, tracking.
- `spacing.css` — 8px scale + layout sizes.
- `effects.css` — radii, rings, shadows, glows, motion.
- `fonts.css` — Inter via Google Fonts.

**Components** (`components/`) — React primitives, exposed on `window.LumibaseDesignSystem_cffa39`
- `core/` — **Button**, **Card**, **Badge**, **Tag**
- `forms/` — **Input**, **Toggle**
- `navigation/` — **PillNav**
- `marketing/` — **FeatureCard**, **ProductHero**

**UI kit** (`ui_kits/`)
- `diagram-landing/` — full lumibase.dev landing-page recreation (`index.html` + `LandingNav`, `Hero`, `ProductSection`, `Footer`).

**Foundations** (`guidelines/`) — specimen cards shown in the Design System tab (Colors, Type, Spacing, Brand).

**Assets** (`assets/`) — logo mark & wordmark, D cut-out, glossy planet renders, galaxy background.
