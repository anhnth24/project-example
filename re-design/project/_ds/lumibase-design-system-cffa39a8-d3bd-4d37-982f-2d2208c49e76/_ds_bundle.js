/* @ds-bundle: {"format":3,"namespace":"LumibaseDesignSystem_cffa39","components":[{"name":"Badge","sourcePath":"components/core/Badge.jsx"},{"name":"Button","sourcePath":"components/core/Button.jsx"},{"name":"Card","sourcePath":"components/core/Card.jsx"},{"name":"Tag","sourcePath":"components/core/Tag.jsx"},{"name":"Input","sourcePath":"components/forms/Input.jsx"},{"name":"Toggle","sourcePath":"components/forms/Toggle.jsx"},{"name":"FeatureCard","sourcePath":"components/marketing/FeatureCard.jsx"},{"name":"ProductHero","sourcePath":"components/marketing/ProductHero.jsx"},{"name":"PillNav","sourcePath":"components/navigation/PillNav.jsx"}],"sourceHashes":{"components/core/Badge.jsx":"ee52ac4293d3","components/core/Button.jsx":"fb50aeb8c3a2","components/core/Card.jsx":"5309463901d2","components/core/Tag.jsx":"cc84c0179c7c","components/forms/Input.jsx":"204ebdab5115","components/forms/Toggle.jsx":"d228042d82d9","components/marketing/FeatureCard.jsx":"8ab5d5a089ed","components/marketing/ProductHero.jsx":"476ddff5f213","components/navigation/PillNav.jsx":"15591c6f1767","components/navigation/liquidGlass.js":"047c75de4a12","ui_kits/diagram-landing/Footer.jsx":"cb0c050add56","ui_kits/diagram-landing/Hero.jsx":"5bf1297a744e","ui_kits/diagram-landing/LandingNav.jsx":"6f378af9dce2","ui_kits/diagram-landing/ProductSection.jsx":"bbd0a899a1b8"},"inlinedExternals":[],"unexposedExports":[{"name":"ensureLiquidGlass","sourcePath":"components/navigation/liquidGlass.js"}]} */

(() => {

const __ds_ns = (window.LumibaseDesignSystem_cffa39 = window.LumibaseDesignSystem_cffa39 || {});

const __ds_scope = {};

(__ds_ns.__errors = __ds_ns.__errors || []);

// components/core/Badge.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * Small pill badge — used on Automator cards to mark a category
 * ("Productivity", "Design Systems"). Subtle glass chip with a dot.
 */
function Badge({
  children,
  tone = "neutral",
  dot = true,
  style = {},
  ...rest
}) {
  const tones = {
    neutral: "var(--color-text-secondary)",
    violet: "var(--color-violet)",
    blue: "var(--color-blue)",
    green: "var(--color-green)"
  };
  const c = tones[tone] || tones.neutral;
  return /*#__PURE__*/React.createElement("span", _extends({
    style: {
      display: "inline-flex",
      alignItems: "center",
      gap: 6,
      height: 24,
      padding: "0 10px",
      borderRadius: "var(--radius-full)",
      background: "var(--color-glass)",
      boxShadow: "var(--ring-glass)",
      font: "var(--text-micro)",
      color: "var(--color-text-secondary)",
      whiteSpace: "nowrap",
      ...style
    }
  }, rest), dot && /*#__PURE__*/React.createElement("span", {
    style: {
      width: 6,
      height: 6,
      borderRadius: "50%",
      background: c,
      boxShadow: `0 0 8px ${c}`
    }
  }), children);
}
Object.assign(__ds_scope, { Badge });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Badge.jsx", error: String((e && e.message) || e) }); }

// components/core/Card.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * Surface card — the dark rounded panel that holds every feature on
 * lumibase.dev. Glass hairline ring, optional violet glow, optional padding.
 */
function Card({
  children,
  glow = "none",
  pad = true,
  surface = "1",
  style = {},
  ...rest
}) {
  const surfaces = {
    "1": "var(--color-surface-1)",
    "2": "var(--color-surface-2)",
    sunken: "var(--color-surface-sunken)",
    violet: "var(--color-surface-violet)"
  };
  const glows = {
    none: "none",
    violet: "var(--glow-violet)",
    blue: "var(--glow-blue)"
  };
  return /*#__PURE__*/React.createElement("div", _extends({
    style: {
      position: "relative",
      borderRadius: "var(--radius-card)",
      background: surfaces[surface] || surfaces["1"],
      boxShadow: glow !== "none" ? `var(--ring-glass), ${glows[glow]}` : "var(--ring-glass)",
      padding: pad ? "var(--space-6)" : 0,
      overflow: "hidden",
      ...style
    }
  }, rest), children);
}
Object.assign(__ds_scope, { Card });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Card.jsx", error: String((e && e.message) || e) }); }

// components/core/Tag.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * Tag chip — solid glass key/value style chip used in toolbars and
 * filter rows (e.g. layer names, model tags). Denser than Badge.
 */
function Tag({
  children,
  active = false,
  icon = null,
  style = {},
  ...rest
}) {
  return /*#__PURE__*/React.createElement("span", _extends({
    style: {
      display: "inline-flex",
      alignItems: "center",
      gap: 6,
      height: 28,
      padding: "0 10px",
      borderRadius: "var(--radius-xs)",
      background: active ? "var(--color-violet)" : "var(--color-surface-3)",
      color: active ? "#fff" : "var(--color-text-secondary)",
      boxShadow: active ? "none" : "var(--ring-glass)",
      font: "600 12px/1 var(--font-sans)",
      whiteSpace: "nowrap",
      ...style
    }
  }, rest), icon, children);
}
Object.assign(__ds_scope, { Tag });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Tag.jsx", error: String((e && e.message) || e) }); }

// components/forms/Input.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * Text input / prompt field — the rounded dark field used in the
 * Magician prompt box and UI-AI playground.
 */
function Input({
  value,
  onChange,
  placeholder = "",
  icon = null,
  trailing = null,
  size = "md",
  style = {},
  ...rest
}) {
  const h = size === "lg" ? 52 : size === "sm" ? 36 : 44;
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      alignItems: "center",
      gap: 10,
      height: h,
      padding: "0 14px",
      borderRadius: "var(--radius-sm)",
      background: "var(--color-surface-sunken)",
      boxShadow: "var(--ring-glass)",
      ...style
    }
  }, icon, /*#__PURE__*/React.createElement("input", _extends({
    value: value,
    onChange: onChange,
    placeholder: placeholder,
    style: {
      flex: 1,
      minWidth: 0,
      background: "transparent",
      border: "none",
      outline: "none",
      color: "var(--color-text)",
      font: "var(--text-body-sm)"
    }
  }, rest)), trailing);
}
Object.assign(__ds_scope, { Input });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/forms/Input.jsx", error: String((e && e.message) || e) }); }

// components/forms/Toggle.jsx
try { (() => {
/**
 * Toggle switch — the pill toggle from the Genius / UI-AI panels.
 * Violet track when on.
 */
function Toggle({
  checked = false,
  onChange,
  disabled = false,
  style = {}
}) {
  return /*#__PURE__*/React.createElement("button", {
    role: "switch",
    "aria-checked": checked,
    disabled: disabled,
    onClick: () => !disabled && onChange && onChange(!checked),
    style: {
      width: 44,
      height: 26,
      flexShrink: 0,
      borderRadius: "var(--radius-full)",
      border: "none",
      cursor: disabled ? "not-allowed" : "pointer",
      opacity: disabled ? 0.5 : 1,
      background: checked ? "var(--color-violet)" : "var(--color-surface-4)",
      boxShadow: checked ? "none" : "var(--ring-glass)",
      position: "relative",
      transition: "background var(--duration) var(--ease-out)",
      ...style
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      position: "absolute",
      top: 3,
      left: checked ? 21 : 3,
      width: 20,
      height: 20,
      borderRadius: "50%",
      background: "#fff",
      boxShadow: "var(--shadow-sm)",
      transition: "left var(--duration) var(--ease-out)"
    }
  }));
}
Object.assign(__ds_scope, { Toggle });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/forms/Toggle.jsx", error: String((e && e.message) || e) }); }

// components/marketing/FeatureCard.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * Feature card — the dark panel with a visual area on top and a
 * title + description below, used throughout the product sections.
 */
function FeatureCard({
  title,
  description,
  visual = null,
  visualHeight = 220,
  align = "bottom",
  glow = "none",
  style = {},
  ...rest
}) {
  return /*#__PURE__*/React.createElement(__ds_scope.Card, _extends({
    glow: glow,
    pad: false,
    style: {
      display: "flex",
      flexDirection: "column",
      ...style
    }
  }, rest), visual && align === "top" && /*#__PURE__*/React.createElement("div", {
    style: {
      position: "relative",
      height: visualHeight,
      overflow: "hidden"
    }
  }, visual), /*#__PURE__*/React.createElement("div", {
    style: {
      padding: "var(--space-6)"
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      font: "var(--text-h3)",
      color: "var(--color-text)",
      letterSpacing: "-0.1px"
    }
  }, title), description && /*#__PURE__*/React.createElement("div", {
    style: {
      font: "var(--text-body-sm)",
      color: "var(--color-text-secondary)",
      marginTop: 8
    }
  }, description)), visual && align === "bottom" && /*#__PURE__*/React.createElement("div", {
    style: {
      position: "relative",
      height: visualHeight,
      overflow: "hidden",
      marginTop: "auto"
    }
  }, visual));
}
Object.assign(__ds_scope, { FeatureCard });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/marketing/FeatureCard.jsx", error: String((e && e.message) || e) }); }

// components/navigation/liquidGlass.js
try { (() => {
/**
 * Injects the shared Liquid Glass SVG filters into the document once.
 * `#dgmLens` is a strong animated displacement (water-droplet refraction);
 * `#dgmLensSoft` is a gentler version for smaller controls (buttons/chips).
 * Apply via CSS `backdrop-filter: blur(..) saturate(..) url(#dgmLens)`.
 */
function ensureLiquidGlass() {
  if (typeof document === "undefined") return;
  if (document.getElementById("dgm-liquid-glass-defs")) return;
  const wrap = document.createElement("div");
  wrap.id = "dgm-liquid-glass-defs";
  wrap.style.cssText = "position:absolute;width:0;height:0;overflow:hidden;pointer-events:none;";
  wrap.innerHTML = `
    <svg xmlns="http://www.w3.org/2000/svg" width="0" height="0" aria-hidden="true">
      <defs>
        <filter id="dgmLens" x="-35%" y="-35%" width="170%" height="170%" color-interpolation-filters="sRGB">
          <feTurbulence type="fractalNoise" baseFrequency="0.009 0.013" numOctaves="2" seed="7" result="noise">
            <animate attributeName="baseFrequency" dur="11s" repeatCount="indefinite"
              values="0.009 0.013; 0.015 0.008; 0.007 0.016; 0.009 0.013" calcMode="spline"
              keySplines="0.45 0 0.55 1; 0.45 0 0.55 1; 0.45 0 0.55 1" />
          </feTurbulence>
          <feGaussianBlur in="noise" stdDeviation="1.6" result="sn" />
          <feDisplacementMap in="SourceGraphic" in2="sn" scale="58"
            xChannelSelector="R" yChannelSelector="G" />
        </filter>
        <filter id="dgmLensSoft" x="-30%" y="-30%" width="160%" height="160%" color-interpolation-filters="sRGB">
          <feTurbulence type="fractalNoise" baseFrequency="0.012 0.016" numOctaves="2" seed="4" result="noise2">
            <animate attributeName="baseFrequency" dur="9s" repeatCount="indefinite"
              values="0.012 0.016; 0.018 0.011; 0.012 0.016" calcMode="spline"
              keySplines="0.45 0 0.55 1; 0.45 0 0.55 1" />
          </feTurbulence>
          <feGaussianBlur in="noise2" stdDeviation="1.4" result="sn2" />
          <feDisplacementMap in="SourceGraphic" in2="sn2" scale="34"
            xChannelSelector="R" yChannelSelector="G" />
        </filter>
      </defs>
    </svg>`;
  document.body.appendChild(wrap);
}
Object.assign(__ds_scope, { ensureLiquidGlass });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/navigation/liquidGlass.js", error: String((e && e.message) || e) }); }

// components/core/Button.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * LumiBase pill button. Glass variant is the brand default (translucent
 * white over the dark cosmic background); solid uses the violet accent.
 */
function Button({
  children,
  variant = "glass",
  size = "md",
  icon = null,
  iconRight = null,
  as = "button",
  disabled = false,
  style = {},
  ...rest
}) {
  const sizes = {
    sm: {
      height: 34,
      padding: "0 14px",
      font: "600 13px/1 var(--font-sans)",
      gap: 6
    },
    md: {
      height: 46,
      padding: "0 22px",
      font: "600 14px/1 var(--font-sans)",
      gap: 8
    },
    lg: {
      height: 54,
      padding: "0 28px",
      font: "600 16px/1 var(--font-sans)",
      gap: 10
    }
  };
  const s = sizes[size] || sizes.md;
  React.useEffect(() => {
    __ds_scope.ensureLiquidGlass();
  }, []);
  const variants = {
    glass: {
      background: "rgba(24,23,28,0.55)",
      color: "var(--color-text)",
      textShadow: "0 1px 2px rgba(0,0,0,0.55)",
      boxShadow: "inset 0 1px 0 rgba(255,255,255,0.28), inset 0 0 0 1px rgba(255,255,255,0.14), inset 0 -7px 14px rgba(0,0,0,0.28), 0 7px 20px -4px rgba(0,0,0,0.55)"
    },
    solid: {
      background: "var(--color-violet)",
      color: "#fff",
      boxShadow: "0 8px 24px -8px rgba(123,97,255,0.7)"
    },
    blue: {
      background: "var(--color-blue)",
      color: "#fff",
      boxShadow: "0 8px 24px -8px rgba(24,160,251,0.7)"
    },
    ghost: {
      background: "transparent",
      color: "var(--color-text-secondary)",
      boxShadow: "none"
    }
  };
  const v = variants[variant] || variants.glass;
  const Tag = as;
  const isGlass = variant === "glass";
  return /*#__PURE__*/React.createElement(Tag, _extends({
    disabled: as === "button" ? disabled : undefined,
    style: {
      position: "relative",
      display: "inline-flex",
      alignItems: "center",
      justifyContent: "center",
      gap: s.gap,
      height: s.height,
      padding: s.padding,
      font: s.font,
      letterSpacing: "0.2px",
      border: "none",
      borderRadius: "var(--radius-pill)",
      overflow: isGlass ? "hidden" : "visible",
      cursor: disabled ? "not-allowed" : "pointer",
      opacity: disabled ? 0.45 : 1,
      textDecoration: "none",
      whiteSpace: "nowrap",
      transition: "filter var(--duration) var(--ease-out), transform 380ms cubic-bezier(0.34,1.56,0.64,1), background var(--duration) var(--ease-out)",
      ...v,
      ...style
    },
    onMouseEnter: e => {
      if (disabled) return;
      e.currentTarget.style.filter = "brightness(1.14)";
      e.currentTarget.style.transform = "translateY(-1px)";
    },
    onMouseLeave: e => {
      e.currentTarget.style.filter = "none";
      e.currentTarget.style.transform = "none";
    },
    onMouseDown: e => {
      if (!disabled) e.currentTarget.style.transform = "translateY(0) scale(0.96)";
    },
    onMouseUp: e => {
      if (!disabled) e.currentTarget.style.transform = "translateY(-1px)";
    }
  }, rest), isGlass && /*#__PURE__*/React.createElement("span", {
    "aria-hidden": true,
    style: {
      position: "absolute",
      inset: 0,
      borderRadius: "var(--radius-pill)",
      zIndex: 0,
      background: "linear-gradient(180deg, rgba(255,255,255,0.10) 0%, rgba(255,255,255,0.02) 100%)",
      backdropFilter: "blur(5px) saturate(185%) url(#dgmLensSoft)",
      WebkitBackdropFilter: "blur(5px) saturate(185%)"
    }
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      position: "relative",
      zIndex: 1,
      display: "inline-flex",
      alignItems: "center",
      gap: s.gap
    }
  }, icon, children, iconRight));
}
Object.assign(__ds_scope, { Button });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Button.jsx", error: String((e && e.message) || e) }); }

// components/marketing/ProductHero.jsx
try { (() => {
/**
 * Product hero / section intro — centred big title, supporting paragraph
 * and a domain pill button. Used to open each product section (Magician,
 * Genius, Automator, UI-AI).
 */
function ProductHero({
  title,
  tagline,
  cta,
  ctaIcon = null,
  note = null,
  align = "center",
  style = {}
}) {
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      flexDirection: "column",
      alignItems: align === "center" ? "center" : "flex-start",
      textAlign: align,
      gap: 0,
      ...style
    }
  }, /*#__PURE__*/React.createElement("h2", {
    style: {
      margin: 0,
      font: "var(--text-h2)",
      letterSpacing: "-0.4px",
      color: "var(--color-text)"
    }
  }, title), tagline && /*#__PURE__*/React.createElement("p", {
    style: {
      margin: "14px 0 0",
      font: "var(--text-lead)",
      color: "var(--color-text-secondary)",
      maxWidth: 460
    }
  }, tagline), cta && /*#__PURE__*/React.createElement("div", {
    style: {
      marginTop: 28
    }
  }, /*#__PURE__*/React.createElement(__ds_scope.Button, {
    variant: "glass",
    icon: ctaIcon
  }, cta)), note && /*#__PURE__*/React.createElement("div", {
    style: {
      marginTop: 14,
      font: "var(--text-caption)",
      color: "var(--color-text-muted)"
    }
  }, note));
}
Object.assign(__ds_scope, { ProductHero });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/marketing/ProductHero.jsx", error: String((e && e.message) || e) }); }

// components/navigation/PillNav.jsx
try { (() => {
/**
 * Floating Liquid Glass pill navigation — Apple-style translucent material.
 * Heavy backdrop blur + saturation, a specular top-edge highlight, soft
 * inner glow, and a sliding glass "lozenge" that sits behind the active item
 * and refracts the background through it.
 */
function PillNav({
  items = [],
  active,
  onSelect,
  style = {}
}) {
  const labels = items.map(it => typeof it === "string" ? it : it.label);
  const wrapRef = React.useRef(null);
  const btnRefs = React.useRef({});
  const [thumb, setThumb] = React.useState({
    left: 0,
    width: 0,
    ready: false
  });
  React.useEffect(() => {
    __ds_scope.ensureLiquidGlass();
  }, []);
  const measure = React.useCallback(() => {
    const el = btnRefs.current[active];
    const wrap = wrapRef.current;
    if (!el || !wrap) return;
    const w = wrap.getBoundingClientRect();
    const b = el.getBoundingClientRect();
    setThumb({
      left: b.left - w.left,
      width: b.width,
      ready: true
    });
  }, [active]);
  React.useLayoutEffect(() => {
    measure();
  }, [measure, labels.join("|")]);
  React.useEffect(() => {
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, [measure]);
  return /*#__PURE__*/React.createElement("nav", {
    ref: wrapRef,
    style: {
      position: "relative",
      display: "inline-flex",
      alignItems: "center",
      gap: 2,
      padding: 5,
      borderRadius: 999,
      overflow: "hidden",
      // clips the displaced glass layer to the pill
      isolation: "isolate",
      boxShadow: ["inset 0 1px 0 rgba(255,255,255,0.6)",
      // top specular edge
      "inset 0 0 0 1px rgba(255,255,255,0.14)",
      // hairline rim
      "inset 1px 0 6px rgba(255,255,255,0.18)",
      // left lens magnify
      "inset -1px 0 6px rgba(255,255,255,0.18)",
      // right lens magnify
      "inset 0 -10px 22px rgba(0,0,0,0.32)",
      // bottom inner shade
      "0 10px 34px -6px rgba(0,0,0,0.6)",
      // ambient drop shadow
      "0 2px 6px rgba(0,0,0,0.35)"].join(", "),
      ...style
    }
  }, /*#__PURE__*/React.createElement("span", {
    "aria-hidden": true,
    style: {
      position: "absolute",
      inset: 0,
      borderRadius: 999,
      zIndex: 0,
      background: "linear-gradient(180deg, rgba(255,255,255,0.14) 0%, rgba(255,255,255,0.05) 48%, rgba(255,255,255,0.02) 100%)",
      backdropFilter: "blur(7px) saturate(185%) url(#dgmLens)",
      WebkitBackdropFilter: "blur(7px) saturate(185%)"
    }
  }), thumb.ready && /*#__PURE__*/React.createElement("span", {
    "aria-hidden": true,
    style: {
      position: "absolute",
      top: 5,
      left: thumb.left,
      width: thumb.width,
      height: "calc(100% - 10px)",
      borderRadius: 999,
      zIndex: 1,
      background: "linear-gradient(180deg, rgba(255,255,255,0.34) 0%, rgba(255,255,255,0.12) 100%)",
      backdropFilter: "blur(1px) saturate(170%) brightness(1.16)",
      WebkitBackdropFilter: "blur(1px) saturate(170%) brightness(1.16)",
      boxShadow: ["inset 0 1px 0 rgba(255,255,255,0.85)", "inset 0 0 0 1px rgba(255,255,255,0.3)", "inset 0 -6px 12px rgba(0,0,0,0.18)", "0 6px 16px -2px rgba(0,0,0,0.45)"].join(", "),
      transition: "left 620ms cubic-bezier(0.34,1.56,0.64,1), width 620ms cubic-bezier(0.34,1.56,0.64,1)"
    }
  }), labels.map(label => {
    const isActive = active === label;
    return /*#__PURE__*/React.createElement("button", {
      key: label,
      ref: n => btnRefs.current[label] = n,
      onClick: () => onSelect && onSelect(label),
      style: {
        position: "relative",
        zIndex: 2,
        height: 38,
        padding: "0 18px",
        border: "none",
        cursor: "pointer",
        background: "transparent",
        borderRadius: 999,
        font: "600 13px/1 var(--font-sans)",
        letterSpacing: "0.2px",
        whiteSpace: "nowrap",
        color: isActive ? "#fff" : "rgba(255,255,255,0.66)",
        textShadow: isActive ? "0 1px 2px rgba(0,0,0,0.35)" : "none",
        transition: "color 240ms cubic-bezier(0.22,1,0.36,1)"
      },
      onMouseEnter: e => {
        if (!isActive) e.currentTarget.style.color = "rgba(255,255,255,0.92)";
      },
      onMouseLeave: e => {
        if (!isActive) e.currentTarget.style.color = "rgba(255,255,255,0.66)";
      }
    }, label);
  }));
}
Object.assign(__ds_scope, { PillNav });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/navigation/PillNav.jsx", error: String((e && e.message) || e) }); }

// ui_kits/diagram-landing/Footer.jsx
try { (() => {
// LumiBase landing — footer
function Footer() {
  const cols = [{
    h: "Product",
    links: ["AI Harness", "Content OS", "Studio", "Runtime"]
  }, {
    h: "Developers",
    links: ["Docs", "SDK", "API Reference", "GitHub"]
  }, {
    h: "Legal",
    links: ["Privacy", "Terms"]
  }];
  return /*#__PURE__*/React.createElement("footer", {
    style: {
      maxWidth: 1200,
      margin: "0 auto",
      padding: "140px 0 56px"
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      justifyContent: "space-between",
      gap: 40
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      maxWidth: 280
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      alignItems: "center",
      gap: 10,
      marginBottom: 14
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      width: 22,
      height: 22,
      borderRadius: "50%",
      background: "linear-gradient(180deg,#fff 0%,#cfcfcf 100%)",
      boxShadow: "0 0 16px rgba(123,97,255,0.6)",
      display: "inline-block"
    }
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      font: "700 18px/1 var(--font-sans)",
      letterSpacing: "-0.4px",
      color: "#fff"
    }
  }, "LumiBase")), /*#__PURE__*/React.createElement("div", {
    style: {
      font: "500 14px/22px var(--font-sans)",
      color: "var(--color-text-muted)",
      marginBottom: 18
    }
  }, "The Content Operating System for the AI era."), /*#__PURE__*/React.createElement("button", {
    style: {
      height: 40,
      padding: "0 18px",
      border: "none",
      cursor: "pointer",
      borderRadius: "var(--radius-pill)",
      background: "var(--color-violet)",
      color: "#fff",
      font: "600 13px/1 var(--font-sans)",
      boxShadow: "0 8px 24px -8px rgba(123,97,255,0.7)"
    }
  }, "Start building")), /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      gap: 64
    }
  }, cols.map(c => /*#__PURE__*/React.createElement("div", {
    key: c.h
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      font: "600 13px/1 var(--font-sans)",
      color: "#fff",
      marginBottom: 16
    }
  }, c.h), /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      flexDirection: "column",
      gap: 12
    }
  }, c.links.map(l => /*#__PURE__*/React.createElement("a", {
    key: l,
    href: "#",
    style: {
      font: "500 14px/1 var(--font-sans)",
      color: "var(--color-text-muted)",
      textDecoration: "none"
    }
  }, l))))))), /*#__PURE__*/React.createElement("div", {
    style: {
      marginTop: 64,
      paddingTop: 24,
      borderTop: "1px solid var(--color-border)",
      display: "flex",
      justifyContent: "space-between",
      alignItems: "center",
      font: "500 13px/1 var(--font-sans)",
      color: "var(--color-text-muted)"
    }
  }, /*#__PURE__*/React.createElement("span", null, "\xA9 2025 LumiBase, Inc."), /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      gap: 18
    }
  }, /*#__PURE__*/React.createElement("span", null, "Twitter"), /*#__PURE__*/React.createElement("span", null, "Discord"), /*#__PURE__*/React.createElement("span", null, "GitHub"))));
}
window.Footer = Footer;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/diagram-landing/Footer.jsx", error: String((e && e.message) || e) }); }

// ui_kits/diagram-landing/Hero.jsx
try { (() => {
// LumiBase landing — hero with orbital solar system
function Hero() {
  const orbits = [{
    size: 266,
    dur: 26,
    planet: "../../assets/planet-red.png",
    p: 22,
    angle: 20
  }, {
    size: 366,
    dur: 34,
    planet: "../../assets/planet-blue.png",
    p: 32,
    angle: 200
  }, {
    size: 520,
    dur: 44,
    planet: "../../assets/planet-green.png",
    p: 26,
    angle: 120
  }, {
    size: 650,
    dur: 60,
    planet: "../../assets/planet-genius.png",
    p: 56,
    angle: 300
  }, {
    size: 820,
    dur: 78,
    planet: "../../assets/planet-magician.png",
    p: 64,
    angle: 60
  }, {
    size: 980,
    dur: 96,
    planet: "../../assets/planet-blue.png",
    p: 20,
    angle: 160
  }];
  return /*#__PURE__*/React.createElement("section", {
    style: {
      position: "relative",
      textAlign: "center",
      paddingTop: 80
    }
  }, /*#__PURE__*/React.createElement("h1", {
    style: {
      margin: 0,
      font: "700 75px/86px var(--font-sans)",
      letterSpacing: "-0.2px",
      color: "#fff"
    }
  }, "Your content,", /*#__PURE__*/React.createElement("br", null), "operated by AI."), /*#__PURE__*/React.createElement("p", {
    style: {
      margin: "24px auto 0",
      maxWidth: 430,
      font: "500 20px/33px var(--font-sans)",
      color: "var(--color-text-secondary)"
    }
  }, "LumiBase is the Content Operating System where agents do the work and you set the intent."), /*#__PURE__*/React.createElement("div", {
    style: {
      marginTop: 32,
      display: "flex",
      justifyContent: "center"
    }
  }, /*#__PURE__*/React.createElement("button", {
    style: {
      height: 46,
      padding: "0 20px 0 16px",
      border: "none",
      cursor: "pointer",
      display: "inline-flex",
      alignItems: "center",
      gap: 9,
      borderRadius: "var(--radius-pill)",
      background: "var(--color-glass)",
      boxShadow: "var(--ring-glass)",
      color: "#fff",
      font: "600 14px/1 var(--font-sans)"
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      width: 20,
      height: 20,
      borderRadius: "50%",
      background: "#fff",
      display: "inline-flex",
      alignItems: "center",
      justifyContent: "center",
      boxShadow: "var(--shadow-sm)"
    }
  }, /*#__PURE__*/React.createElement("img", {
    src: "../../assets/logo-mark.svg",
    style: {
      width: 12,
      height: 12,
      filter: "invert(1)"
    }
  })), "Read the docs")), /*#__PURE__*/React.createElement("div", {
    style: {
      position: "relative",
      width: 980,
      height: 980,
      margin: "10px auto 0"
    }
  }, orbits.map((o, i) => /*#__PURE__*/React.createElement("div", {
    key: i,
    style: {
      position: "absolute",
      left: "50%",
      top: "50%",
      width: o.size,
      height: o.size,
      marginLeft: -o.size / 2,
      marginTop: -o.size / 2,
      borderRadius: "50%",
      border: "1px solid rgba(255,255,255,0.10)",
      animation: `spin ${o.dur}s linear infinite`
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      position: "absolute",
      left: "50%",
      top: 0,
      transform: `translateX(-50%)`,
      width: o.p,
      height: o.p,
      marginTop: -o.p / 2,
      animation: `spin ${o.dur}s linear infinite reverse`
    }
  }, /*#__PURE__*/React.createElement("img", {
    src: o.planet,
    style: {
      width: o.p,
      height: o.p,
      display: "block",
      filter: "drop-shadow(0 6px 14px rgba(0,0,0,0.5))"
    }
  })))), /*#__PURE__*/React.createElement("div", {
    style: {
      position: "absolute",
      left: "50%",
      top: "50%",
      width: 190,
      height: 190,
      marginLeft: -95,
      marginTop: -95,
      borderRadius: "50%",
      background: "linear-gradient(180deg,#fff 0%,#cfcfcf 100%)",
      boxShadow: "0 0 80px rgba(123,97,255,0.35), var(--shadow-lg)",
      overflow: "hidden"
    }
  }, /*#__PURE__*/React.createElement("img", {
    src: "../../assets/d-cutout.png",
    style: {
      position: "absolute",
      left: 92,
      top: 24,
      width: 72,
      height: 140
    }
  }))));
}
window.Hero = Hero;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/diagram-landing/Hero.jsx", error: String((e && e.message) || e) }); }

// ui_kits/diagram-landing/LandingNav.jsx
try { (() => {
// LumiBase landing — top nav (logo + floating pill nav + login)
function LandingNav({
  tab,
  setTab
}) {
  const {
    PillNav
  } = window.LumibaseDesignSystem_cffa39;
  return /*#__PURE__*/React.createElement("div", {
    style: {
      position: "sticky",
      top: 0,
      zIndex: 50,
      height: 72,
      display: "flex",
      alignItems: "center",
      justifyContent: "space-between",
      padding: "0 40px"
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      alignItems: "center",
      gap: 10
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      width: 24,
      height: 24,
      borderRadius: "50%",
      background: "linear-gradient(180deg,#fff 0%,#cfcfcf 100%)",
      boxShadow: "0 0 18px rgba(123,97,255,0.6), var(--shadow-sm)",
      display: "inline-block",
      flexShrink: 0
    }
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      font: "700 19px/1 var(--font-sans)",
      letterSpacing: "-0.4px",
      color: "#fff"
    }
  }, "LumiBase")), /*#__PURE__*/React.createElement("div", {
    style: {
      position: "absolute",
      left: "50%",
      transform: "translateX(-50%)"
    }
  }, /*#__PURE__*/React.createElement(PillNav, {
    items: ["AI Harness", "Content OS", "Studio", "Runtime"],
    active: tab,
    onSelect: setTab
  })), /*#__PURE__*/React.createElement("button", {
    style: {
      height: 38,
      padding: "0 18px",
      border: "none",
      cursor: "pointer",
      borderRadius: "var(--radius-pill)",
      background: "var(--color-glass)",
      boxShadow: "var(--ring-glass)",
      color: "#fff",
      font: "600 13px/1 var(--font-sans)",
      letterSpacing: "0.2px"
    }
  }, "Login"));
}
window.LandingNav = LandingNav;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/diagram-landing/LandingNav.jsx", error: String((e && e.message) || e) }); }

// ui_kits/diagram-landing/ProductSection.jsx
try { (() => {
// LumiBase landing — reusable product section (Magician, Genius, Automator, UI-AI)
function ProductSection({
  id,
  planet,
  glow,
  title,
  tagline,
  domain,
  note,
  features
}) {
  const {
    ProductHero,
    FeatureCard,
    Badge
  } = window.LumibaseDesignSystem_cffa39;
  return /*#__PURE__*/React.createElement("section", {
    id: id,
    "data-screen-label": title,
    style: {
      maxWidth: 1200,
      margin: "0 auto",
      padding: "120px 0 0"
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: "flex",
      flexDirection: "column",
      alignItems: "center"
    }
  }, /*#__PURE__*/React.createElement("img", {
    src: planet,
    style: {
      width: 96,
      height: 96,
      filter: `drop-shadow(0 0 44px ${glow})`,
      marginBottom: 26
    }
  }), /*#__PURE__*/React.createElement(ProductHero, {
    title: title,
    tagline: tagline,
    cta: domain,
    note: note,
    ctaIcon: /*#__PURE__*/React.createElement("span", {
      style: {
        width: 8,
        height: 8,
        borderRadius: "50%",
        background: glow
      }
    })
  })), /*#__PURE__*/React.createElement("div", {
    style: {
      display: "grid",
      gridTemplateColumns: "1fr 1fr 1fr",
      gap: 16,
      marginTop: 64
    }
  }, features.map((f, i) => /*#__PURE__*/React.createElement("div", {
    key: i,
    style: {
      gridColumn: f.span ? `span ${f.span}` : "span 1"
    }
  }, /*#__PURE__*/React.createElement(FeatureCard, {
    glow: f.glow || "none",
    title: f.title,
    description: f.desc,
    visualHeight: f.vh || 190,
    visual: /*#__PURE__*/React.createElement("div", {
      style: {
        position: "absolute",
        inset: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        background: f.bg || "transparent"
      }
    }, f.badge && /*#__PURE__*/React.createElement(Badge, {
      tone: f.badgeTone || "violet"
    }, f.badge), f.img && /*#__PURE__*/React.createElement("img", {
      src: f.img,
      style: {
        width: f.imgW || 120,
        height: f.imgW || 120,
        filter: `drop-shadow(0 0 36px ${glow})`
      }
    }), f.node)
  })))));
}
window.ProductSection = ProductSection;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/diagram-landing/ProductSection.jsx", error: String((e && e.message) || e) }); }

__ds_ns.Badge = __ds_scope.Badge;

__ds_ns.Button = __ds_scope.Button;

__ds_ns.Card = __ds_scope.Card;

__ds_ns.Tag = __ds_scope.Tag;

__ds_ns.Input = __ds_scope.Input;

__ds_ns.Toggle = __ds_scope.Toggle;

__ds_ns.FeatureCard = __ds_scope.FeatureCard;

__ds_ns.ProductHero = __ds_scope.ProductHero;

__ds_ns.PillNav = __ds_scope.PillNav;

})();
