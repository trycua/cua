"use strict";
const fs = require("fs"); const path = require("path");
class ManualStore {
  constructor(workspace) { this.workspace = path.resolve(workspace); this.articlesPath = path.join(this.workspace, "wiki", "articles.json"); }
  read() { return JSON.parse(fs.readFileSync(this.articlesPath, "utf8")); }
  list() { return this.read().articles.map(function (article) { return { id: article.id, title: article.title, summary: article.summary }; }); }
  get(id) { const article = this.read().articles.find(function (item) { return item.id === id; }); if (!article) { throw new Error("unknown article " + id); } return article; }
}
module.exports = { ManualStore };
