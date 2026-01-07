const chokidar = require("chokidar");
const build = require("./build");
const express = require("express");
const fs = require("fs");
const path = require("path");

const app = express();
const PORT = 3000;

// Load iconset data
function loadIconsetData() {
  const iconsets = {};
  const iconsetDir = path.join(__dirname, "../cua_bench/www/iconsets");
  
  if (!fs.existsSync(iconsetDir)) {
    console.warn("⚠️  Iconsets directory not found:", iconsetDir);
    return iconsets;
  }
  
  const files = fs.readdirSync(iconsetDir);
  files.forEach(file => {
    if (file.endsWith(".json")) {
      const themeName = file.replace(".json", "");
      try {
        const data = JSON.parse(fs.readFileSync(path.join(iconsetDir, file), "utf-8"));
        iconsets[themeName] = data;
      } catch (err) {
        console.error(`Failed to load ${file}:`, err.message);
      }
    }
  });
  
  return iconsets;
}

// Serve static files from dist
app.use("/dist", express.static(path.join(__dirname, "dist")));

// Main route - serve example.html with iconset data injected
app.get("/", (req, res) => {
  const iconsets = loadIconsetData();
  
  // Read example.html
  const htmlPath = path.join(__dirname, "example.html");
  let html = fs.readFileSync(htmlPath, "utf-8");
  
  // Inject iconset data as a script tag
  const iconsetScript = `
    <script>
      window.ICONSETS = ${JSON.stringify(iconsets, null, 2)};
    </script>
  `;
  
  // Insert before closing </head> tag
  html = html.replace("</head>", `${iconsetScript}\n</head>`);
  
  res.send(html);
});

// LiveReload SSE endpoint
let clients = [];
app.get("/livereload", (req, res) => {
  res.writeHead(200, {
    "Content-Type": "text/event-stream",
    "Cache-Control": "no-cache",
    "Connection": "keep-alive",
  });
  
  clients.push(res);
  
  req.on("close", () => {
    clients = clients.filter(client => client !== res);
  });
});

function notifyClients() {
  clients.forEach(client => {
    client.write("data: reload\n\n");
  });
}

// Watch for changes and rebuild
chokidar
  .watch(["gui", "example.html", "build.js"], {
    usePolling: true,
    ignoreInitial: true,
  })
  .on("change", (file) => {
    console.log(
      `[${new Date().toLocaleTimeString()}] ${file} changed -- rebuilding...`
    );
    build().then(() => {
      console.log("✓ Rebuild complete, reloading browser...");
      notifyClients();
    });
  });

// Initial build on startup
console.log("🚀 Starting development server...");
build().then(() => {
  app.listen(PORT, () => {
    console.log(`✓ Server running at http://localhost:${PORT}`);
    console.log(`✓ Hot reload enabled`);
    console.log(`✓ Loaded iconsets: ${Object.keys(loadIconsetData()).join(", ")}`);
    console.log("\n💡 Edit SCSS files in gui/[theme]/ and save to see changes");
  });
});
