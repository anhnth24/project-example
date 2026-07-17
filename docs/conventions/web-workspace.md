# Web workspace policy

The repository uses a root pnpm workspace containing `app/` (Tauri desktop) and
`web/` (browser SPA). Node must be 20+ and pnpm is pinned to `10.33.3`.

```bash
bash scripts/check-web-toolchain.sh
pnpm install --frozen-lockfile
pnpm --filter markhand-web build
pnpm --filter markhand-desktop build
```

Root task aliases arrive in F-09; do not add a second package manager, per-package
lockfile or ad-hoc task runner. The Compose requirement is established here, but the
actual CPU-only stack is owned by F-08.
