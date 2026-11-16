"use strict";

const appRoot = document.getElementById("app");
const messageList = document.getElementById("messages");
const reader = document.getElementById("reader");
const count = document.getElementById("message-count");
const status = document.getElementById("status");
const dialog = document.getElementById("attachment-dialog");
const attachmentTitle = document.getElementById("attachment-title");
const attachmentContent = document.getElementById("attachment-content");
const closeAttachment = document.getElementById("close-attachment");

function element(tag, attributes, children) {
  const node = document.createElement(tag);
  Object.keys(attributes || {}).forEach(function (key) {
    if (key === "className") node.className = attributes[key];
    else node.setAttribute(key, attributes[key]);
  });
  (children || []).forEach(function (child) {
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  });
  return node;
}

function setStatus(message, error) {
  status.textContent = message;
  status.classList.toggle("error", Boolean(error));
}

function displayDate(value) {
  return new Intl.DateTimeFormat("en-US", {
    month: "short", day: "numeric", hour: "numeric", minute: "2-digit",
    timeZone: "America/Chicago",
  }).format(new Date(value));
}

async function showAttachment(message, metadata) {
  try {
    const attachment = await window.ledgerLantern.openAttachment(message.id, metadata.id);
    attachmentTitle.textContent = attachment.name;
    attachmentContent.textContent = attachment.content;
    dialog.showModal();
    attachmentContent.focus();
    setStatus("Opened " + attachment.name + " from the immutable source archive.");
  } catch (error) {
    setStatus(error.message, true);
  }
}

function showMessage(message, button) {
  document.querySelectorAll(".message-button").forEach(function (item) {
    item.classList.toggle("selected", item === button);
    item.setAttribute("aria-pressed", String(item === button));
  });

  const labels = element("div", { className: "labels", "aria-label": "Message labels" },
    message.labels.map(function (label) { return element("span", {}, [label]); }));
  const attachments = element("div", { className: "attachments" });
  if (message.attachments.length) {
    attachments.append(element("h3", {}, ["Attachments"]));
    message.attachments.forEach(function (metadata) {
      const open = element("button", {
        type: "button",
        className: "attachment",
        "aria-label": "Open read-only attachment " + metadata.name,
      }, [element("span", { className: "file-icon", "aria-hidden": "true" }, ["TXT"]),
        element("span", {}, [element("strong", {}, [metadata.name]), element("small", {}, ["Open preview · read-only"])])]);
      open.addEventListener("click", function () { showAttachment(message, metadata); });
      attachments.append(open);
    });
  }

  reader.replaceChildren(
    element("div", { className: "reader-heading" }, [
      labels,
      element("h2", {}, [message.subject]),
      element("div", { className: "sender" }, [
        element("div", { className: "avatar", "aria-hidden": "true" }, [message.from.split(/\s+/).map(function (part) { return part[0]; }).join("").slice(0, 2)]),
        element("div", {}, [element("strong", {}, [message.from]), element("span", {}, [message.from_address])]),
        element("time", { datetime: message.sent_at }, [displayDate(message.sent_at)]),
      ]),
      element("p", { className: "recipient" }, ["To: " + message.to]),
    ]),
    element("div", { className: "message-body" }, message.body.split("\n").map(function (paragraph) {
      return element("p", {}, [paragraph]);
    })),
    attachments,
    element("p", { className: "archive-note" }, ["This message and its attachments are source records. Editing, replying, forwarding, and downloading are disabled."])
  );
  setStatus("Opened " + message.subject + ". Source record is read-only.");
}

function messageButton(message) {
  const button = element("button", { type: "button", className: "message-button", "aria-pressed": "false" }, [
    element("span", { className: "message-topline" }, [element("strong", {}, [message.from]), element("time", { datetime: message.sent_at }, [displayDate(message.sent_at)])]),
    element("span", { className: "subject" }, [message.subject]),
    element("span", { className: "snippet" }, [message.body.replace(/\s+/g, " ").slice(0, 88)]),
    element("span", { className: "attachment-count" }, [message.attachments.length + " attachment" + (message.attachments.length === 1 ? "" : "s")]),
  ]);
  button.addEventListener("click", function () { showMessage(message, button); });
  return button;
}

async function start() {
  try {
    const messages = await window.ledgerLantern.listMessages();
    count.textContent = String(messages.length);
    const buttons = messages.map(messageButton);
    buttons.forEach(function (button) { messageList.append(element("li", {}, [button])); });
    if (messages.length) showMessage(messages[0], buttons[0]);
    else reader.replaceChildren(element("p", { className: "empty" }, ["No source messages found."]));
    setStatus(messages.length + " immutable source message" + (messages.length === 1 ? "" : "s") + " loaded.");
  } catch (error) {
    reader.replaceChildren(element("h2", {}, ["Unable to load source mail"]), element("p", {}, [error.message]));
    setStatus(error.message, true);
  } finally {
    appRoot.setAttribute("aria-busy", "false");
  }
}

closeAttachment.addEventListener("click", function () { dialog.close(); });
dialog.addEventListener("click", function (event) {
  if (event.target === dialog) dialog.close();
});

start();
