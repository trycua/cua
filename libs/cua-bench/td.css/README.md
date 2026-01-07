## td.css

[![npm](https://img.shields.io/npm/v/xp.css)](http://npm.im/xp.css)
[![gzip size](https://img.shields.io/bundlephobia/minzip/xp.css)](https://unpkg.com/xp.css)

A design system for building faithful recreations of old UIs.

<img alt="a screenshot of a window with the title 'My First Program' and two buttons OK and Cancel, styled like a Windows XP dialog" src="https://github.com/botoxparty/XP.css/blob/main/docs/window.png?raw=true" height="133">

<img alt="a screenshot of a window with the title 'My First Program' and two buttons OK and Cancel, styled like a Windows 98 dialog" src="https://github.com/jdan/98.css/blob/main/docs/window.png?raw=true" height="133">

td.css is based on XP.css, adding themes for win11, win10, win7, macOS, Android, and iOS.

XP.css is an extension of 98.css. A CSS file that takes semantic HTML and makes it look pretty. It does not ship with any JavaScript, so it is compatible with your frontend framework of choice.

### Supported OS Themes

- **Windows 11** (`win11`, `win11:light`, `win11:dark`)
- **Windows 10** (`win10`)
- **Windows 7** (`win7`)
- **macOS** (`macos`, `macos:light`, `macos:dark`)
- **Windows XP** (`winxp`)
- **Windows 98** (`win98`)
- **Android** (`android`)
- **iOS** (`ios`, `ios:light`, `ios:dark`)

### Building td.css

The td.css design system is located in the `td.css/` directory. To build it:

```bash
cd td.css
npm install

# Build for distribution (outputs to dist/td.css)
npm run build

# Build for cua-bench (outputs to ../cua_bench/www/td.css)
npm run build:cua-bench
```

This compiles the SCSS files from `gui/index.scss` into a single `td.css` file which includes all OS themes (Windows 11, Windows 10, Windows 7, macOS, Windows XP, Windows 98, Android, iOS).
