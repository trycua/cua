#!/usr/bin/env node
const fs = require("fs");
const path = require("path");
const mkdirp = require("mkdirp");
const postcss = require("postcss");

const { homepage, version } = require("./package.json");

// Check if building for cua-bench
const iscuaBench = process.argv.includes("--cua-bench");
const outputDir = iscuaBench ? "../cua_bench/www/css" : "dist";

// Function to copy iconsets
function copyIconsets() {
  const iconsetSource = path.join(__dirname, "../cua_bench/www/iconsets");
  const iconsetDest = path.join(outputDir, "iconsets");
  
  // Check if source iconsets directory exists
  if (!fs.existsSync(iconsetSource)) {
    console.log("⚠️  Iconsets directory not found, skipping...");
    return;
  }
  
  // Create destination directory
  mkdirp.sync(iconsetDest);
  
  // Copy all files from iconsets
  const files = fs.readdirSync(iconsetSource);
  let copiedCount = 0;
  
  files.forEach(file => {
    const srcPath = path.join(iconsetSource, file);
    const destPath = path.join(iconsetDest, file);
    
    if (fs.statSync(srcPath).isFile()) {
      fs.copyFileSync(srcPath, destPath);
      copiedCount++;
    }
  });
  
  if (copiedCount > 0) {
    console.log(`📋 Copied ${copiedCount} iconset file(s)`);
  }
}

const postcssParser = postcss()
  .use(require("postcss-import"))
  .use(require("postcss-strip-inline-comments"))
  .use(require("postcss-simple-vars"))
  .use(require("postcss-nested"))
  .use(require("postcss-inline-svg")({
    paths: ['gui']
  }))
  .use(require("postcss-custom-properties")({
    preserve: false
  }))
  .use(require("postcss-calc"))
  .use(require("postcss-copy")({ 
    dest: outputDir, 
    template: "[name].[ext]",
    // Exclude SVG files from being copied since they'll be inlined
    ignore: ['**/*.svg']
  }));

// Theme configurations
const themes = [
  { name: "win11", path: "gui/win11/index.scss", output: "win11.css" },
  { name: "win11_light", path: "gui/win11/light.scss", output: "win11_light.css", base: "gui/win11/index.scss" },
  { name: "win10", path: "gui/win10/index.scss", output: "win10.css" },
  { name: "win7", path: "gui/win7/index.scss", output: "win7.css" },
  { name: "macos", path: "gui/macos/style.scss", output: "macos.css" },
  { name: "macos_light", path: "gui/macos/style.light.scss", output: "macos_light.css", base: "gui/macos/style.scss" },
  { name: "winxp", path: "gui/winXP/index.scss", output: "winxp.css" },
  { name: "win98", path: "gui/win98/index.scss", output: "win98.css" },
  { name: "android", path: "gui/android/style.scss", output: "android.css" },
  { name: "ios", path: "gui/ios/style.scss", output: "ios.css" },
  { name: "ios_light", path: "gui/ios/light.scss", output: "ios_light.css", base: "gui/ios/style.scss" },
];

function buildTheme(theme) {
  // Read the theme file directly
  let input = `/*! ${theme.name}.css v${version} - ${homepage} */\n`;
  
  // If theme has a base, import it first
  if (theme.base) {
    input += fs.readFileSync(theme.base, 'utf-8') + '\n';
  }
  
  // Import the theme
  input += fs.readFileSync(theme.path, 'utf-8') + '\n';

  // Add globals
  (['gui/_app_icons.scss', 'gui/_globals.scss', 'gui/_iconify_icons.scss']).forEach((file) => {
    input += fs.readFileSync(file, 'utf-8') + '\n';
  });

  return postcssParser
    .process(input, {
      from: theme.path,
      to: `${outputDir}/${theme.output}`,
      map: { inline: false },
      syntax: require('postcss-scss')
    })
    .then((result) => {
      mkdirp.sync(outputDir);
      fs.writeFileSync(`${outputDir}/${theme.output}`, result.css);
      fs.writeFileSync(`${outputDir}/${theme.output}.map`, result.map.toString());
      console.log(`✓ Built ${theme.output}`);
    })
    .catch((err) => {
      console.error(`❌ Failed to build ${theme.name}:`, err.message);
      throw err;
    });
}

async function build() {
  console.log(`Building ${themes.length} themes...\n`);
  
  try {
    // Build all themes in parallel
    await Promise.all(themes.map(theme => buildTheme(theme)));
    
    // Copy iconsets
    copyIconsets();
    
    console.log("\n✅ Build complete!");
    if (iscuaBench) {
      console.log(`📦 Output: ${outputDir}/`);
      console.log(`💡 ${themes.length} theme files ready for use in cua-bench`);
    } else {
      console.log(`📦 Output: dist/`);
    }
    console.log(`\nThemes: ${themes.map(t => t.output).join(', ')}`);
  } catch (err) {
    console.error("\n❌ Build failed:", err);
    process.exit(1);
  }
}

module.exports = build;

build();
