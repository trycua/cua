"use strict";

const fs = require("node:fs");
const path = require("node:path");

function compare(left, right) {
  const a = String(left === undefined || left === null ? "" : left);
  const b = String(right === undefined || right === null ? "" : right);
  return a < b ? -1 : (a > b ? 1 : 0);
}

function requiredText(value, field) {
  const result = String(value === undefined || value === null ? "" : value).trim();
  if (!result) throw new Error(field + " is required");
  return result;
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function messagesFrom(document) {
  if (!document || !Array.isArray(document.messages)) {
    throw new Error("messages.json must contain a messages array");
  }
  return document.messages;
}

function publicAttachment(attachment) {
  return {
    id: requiredText(attachment.id, "attachment.id"),
    name: requiredText(attachment.name, "attachment.name"),
    media_type: requiredText(attachment.media_type, "attachment.media_type"),
  };
}

function publicMessage(message) {
  return {
    id: requiredText(message.id, "message.id"),
    from: requiredText(message.from, "message.from"),
    from_address: requiredText(message.from_address, "message.from_address"),
    to: requiredText(message.to, "message.to"),
    sent_at: requiredText(message.sent_at, "message.sent_at"),
    subject: requiredText(message.subject, "message.subject"),
    body: requiredText(message.body, "message.body"),
    labels: Array.isArray(message.labels) ? message.labels.map(String) : [],
    attachments: Array.isArray(message.attachments) ? message.attachments.map(publicAttachment) : [],
    read_only: true,
  };
}

class MailStore {
  constructor(workspace) {
    this.mailDirectory = path.resolve(workspace, "mail");
    this.messagesPath = path.join(this.mailDirectory, "messages.json");
    this.attachmentsDirectory = path.join(this.mailDirectory, "attachments");
  }

  sourceMessages() {
    return messagesFrom(readJson(this.messagesPath));
  }

  list() {
    return this.sourceMessages().map(publicMessage).sort(function (left, right) {
      return compare(right.sent_at, left.sent_at) || compare(left.id, right.id);
    });
  }

  attachment(messageId, attachmentId) {
    const wantedMessage = requiredText(messageId, "message id");
    const wantedAttachment = requiredText(attachmentId, "attachment id");
    const message = this.sourceMessages().find(function (item) { return item.id === wantedMessage; });
    if (!message) throw new Error("unknown message " + wantedMessage);
    const attachments = Array.isArray(message.attachments) ? message.attachments : [];
    const metadata = attachments.find(function (item) { return item.id === wantedAttachment; });
    if (!metadata) throw new Error("unknown attachment " + wantedAttachment + " for message " + wantedMessage);

    const relativeFile = requiredText(metadata.file, "attachment.file");
    const resolvedFile = path.resolve(this.attachmentsDirectory, relativeFile);
    const relative = path.relative(this.attachmentsDirectory, resolvedFile);
    if (!relative || relative.startsWith(".." + path.sep) || path.isAbsolute(relative)) {
      throw new Error("attachment file must stay inside the mail attachments directory");
    }
    const metadataStat = fs.lstatSync(resolvedFile);
    const realRoot = fs.realpathSync(this.attachmentsDirectory);
    const realFile = fs.realpathSync(resolvedFile);
    const realRelative = path.relative(realRoot, realFile);
    if (
      metadataStat.isSymbolicLink()
      || !metadataStat.isFile()
      || !realRelative
      || realRelative.startsWith(".." + path.sep)
      || path.isAbsolute(realRelative)
      || metadataStat.size > 1024 * 1024
    ) {
      throw new Error("attachment must be a bounded regular file inside the mail archive");
    }

    return Object.assign(publicAttachment(metadata), {
      content: fs.readFileSync(resolvedFile, "utf8"),
      read_only: true,
    });
  }
}

module.exports = { MailStore };
