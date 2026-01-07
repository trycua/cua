# td.css Development Guide

This guide will help you develop and test td.css themes with hot reload and automatic icon templating.

## Prerequisites

- Node.js (v12 or higher)
- pnpm (install with `npm install -g pnpm`)

## Installation

Install dependencies:

```bash
pnpm install
```

## Development Workflow (Recommended)

The development server provides hot reload and automatic icon templating:

1. **Start the development server:**
   ```bash
   pnpm run dev
   ```
   or
   ```bash
   node server.js
   ```

2. **Open your browser:**
   Navigate to `http://localhost:3000`

3. **Make changes:**
   Edit SCSS files in `gui/[theme]/` and save

4. **See changes automatically:**
   The page will rebuild and reload automatically - no manual refresh needed!

### What the Development Server Does

- **Hot Reload**: Automatically rebuilds CSS and reloads the browser when you save changes
- **Iconset Templating**: Loads iconset data from `../cua_bench/www/iconsets/*.json` and injects it into the page
- **Dynamic Desktop**: Generates random taskbar, dock, and desktop icons matching the webtop.py behavior
- **Live Preview**: See all themes with realistic desktop environments

## Manual Build Workflow

For manual building without the dev server:

1. **Build the CSS:**
   ```bash
   pnpm run build
   ```

2. **Open example.html directly:**
   ```bash
   open dist/example.html
   ```
   Or double-click `example.html` in your file browser.

3. **Rebuild after changes:**
   ```bash
   pnpm run build
   ```
   Then refresh your browser manually.

## Features

The `example.html` file includes:

- **Theme Switcher:** Preview all OS themes (macOS, Windows, iOS, Android)
- **Resolution Selector:** Test different screen sizes from mobile to 4K
- **Live Desktop:** Realistic desktop environment matching webtop.py
- **Dynamic Icons:** Automatically loads and displays random icons from iconsets
- **Taskbar/Dock:** Windows taskbar and macOS dock populated with random apps
- **Desktop Icons:** 0-25 random desktop icons like the real provider
- **Hot Reload:** Automatic page refresh when files change (dev server only)

## Building for cua-bench

To build directly to the cua-bench www directory:

```bash
pnpm run build:cua-bench
```

This outputs to `../cua_bench/www/css/` instead of `dist/`.

## Theme Structure

Each theme is located in `gui/[theme]/`:

- `gui/win11/` - Windows 11 theme (dark + light)
- `gui/win10/` - Windows 10 theme
- `gui/win7/` - Windows 7 theme
- `gui/macos/` - macOS theme (dark + light)
- `gui/winXP/` - Windows XP theme
- `gui/win98/` - Windows 98 theme
- `gui/android/` - Android theme
- `gui/ios/` - iOS theme (dark + light)

## Adding New Themes

To add a new theme easily:

1. **Create iconset JSON** in `../cua_bench/www/iconsets/[theme].json`:
   ```json
   {
     "icons": {
       "system_icons": ["Icon1", "Icon2", ...],
       "application_icons": ["App1", "App2", ...]
     }
   }
   ```

2. **Create iconset CSS** in `../cua_bench/www/iconsets/[theme].css`:
   Define your icon styles using `.icon[data-icon="IconName"]`

3. **Create theme SCSS** in `gui/[theme]/index.scss`:
   Define your theme styles

4. **Add to build.js**:
   Add your theme configuration to the `themes` array

5. **Test with dev server**:
   ```bash
   pnpm run dev
   ```
   Your theme will automatically load the iconset data and display random icons!

## How Iconset Templating Works

The development server (`server.js`) automatically:

1. Loads all JSON files from `../cua_bench/www/iconsets/`
2. Injects the data as `window.ICONSETS` into example.html
3. JavaScript in example.html uses this data to:
   - Generate random desktop icons (0-25 icons)
   - Populate the macOS dock (3-7 apps + trash)
   - Populate the Windows taskbar (2-5 pinned apps)

This matches the behavior of `providers/webtop.py` for realistic testing.

## Troubleshooting

**Dev server fails to start:**
- Make sure all dependencies are installed: `pnpm install`
- Check that port 3000 is not in use
- Verify Node.js is installed: `node --version`

**Build fails:**
- Make sure all dependencies are installed: `pnpm install`
- Check that Node.js is installed: `node --version`

**Hot reload not working:**
- Make sure you're accessing via `http://localhost:3000`, not opening the file directly
- Check the browser console for errors
- Try hard refresh (Cmd+Shift+R on Mac, Ctrl+Shift+R on Windows)

**Icons not showing:**
- Verify iconset JSON files exist in `../cua_bench/www/iconsets/`
- Check browser console for errors
- Ensure iconset CSS files are built and loaded

**Changes not showing:**
- Wait for the rebuild message in the terminal
- Clear your browser cache if needed
- Check that you're editing the correct SCSS file
- Verify the file is being watched (should be in `gui/` directory)

## Tips

- Use the dev server for the best experience
- Changes to SCSS files trigger automatic rebuilds
- Desktop icons are randomized on each theme switch
- Resolution and theme preferences are saved in localStorage
- The example desktop closely mirrors what users see in cua-bench
