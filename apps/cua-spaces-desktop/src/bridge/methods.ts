// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The methods this host answers: the SwiftUI host's (`WebUIBridge.methods`,
// in its order, which `app.info` lists), plus what only a host that draws
// video in the page answers (`ELECTRON_HOST_METHODS` in the web bridge's
// webkit-protocol.ts). `test/bridge-registry.test.ts` checks this list
// against the Swift source, the web contract and the registry.

export const METHODS = [
  "app.info",
  "session.get", "session.signIn", "session.signOut",
  "spaces.list", "spaces.open",
  "spaces.createOptions", "spaces.create",
  "spaces.setPower", "spaces.delete", "spaces.cancelCreate",
  "machines.list", "host.status",
  "agents.list", "agents.runs", "agents.events", "agents.pause", "agents.resume",
  "agents.setup", "agents.configure",
  "agentKeys.list", "agentKeys.set", "agentKeys.remove",
  "spaces.add", "clouds.status", "clouds.test", "clouds.connect",
  "teleport.catalog", "teleport.entryForPath", "teleport.windows", "teleport.remoteWindows",
  "teleport.icon", "teleport.thumbnail", "teleport.plan", "teleport.run", "teleport.sites",
  "teleport.remembered", "teleport.streamWindow", "sharing.list", "sharing.share", "sharing.unshare",
  "volume.overview", "volume.storage", "volume.storageSet", "volume.mount", "volume.unmount",
  "volume.approve", "volume.deny", "volume.revoke", "volume.resolve", "volume.reveal",
  "agents.setupDriver",
  "about.get", "about.set", "about.checkNow", "loginItem.get", "loginItem.set", "loginItem.openSettings",
  "devices.get", "devices.enroll", "devices.checkEnrolled", "devices.approve", "devices.rename",
  "devices.revoke", "devices.confirmMachine", "storage.get", "storage.run",
  "notifications.list", "notifications.markAllRead",
  "telemetry.track", "spaces.usage", "spaces.windows", "stream.pip",
  "spaces.thumbnail",
  "spaces.chooseFiles", "spaces.droppedFiles", "spaces.sendFiles",
  "host.setUp", "host.action", "host.openSettings",
  "settings.get", "settings.choose",
  "keyvault.get", "keyvault.lock", "keyvault.unlock",
  "keyvault.unlockVault", "keyvault.setup", "keyvault.setDisabled",
  "keyvault.approve", "keyvault.deny", "keyvault.revokeGrant",
  "keyvault.showItems", "keyvault.delete", "keyvault.run", "keyvault.dismiss",
  "window.setBackgroundColor", "window.setDragRegions",
  "startup.get", "startup.act",
] as const;

/** A live desktop's media ticket for the page's WebCodecs player, and the
 * first run's state (the SwiftUI app draws its first run natively). */
export const ELECTRON_METHODS = ["onboarding.get", "onboarding.complete", "spaces.openStream"] as const;

export type Method = (typeof METHODS)[number] | (typeof ELECTRON_METHODS)[number];
