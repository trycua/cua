// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import ExpoModulesCore
import Security

/// `call(method, argsJson)` over the Rust app core: the same dispatcher the
/// web app loads as wasm and the SwiftUI app reaches through UniFFI.
public class CuaCoreModule: Module {
  public func definition() -> ModuleDefinition {
    Name("CuaCore")

    Constants(["available": true])

    // Synchronous: a core call is pure view-model logic (microseconds).
    Function("call") { (method: String, args: String) throws -> String in
      var err: NSString?
      guard let out = CuaCoreCall(method, args, &err) else {
        throw CoreError(err as String? ?? "core error")
      }
      return out
    }
    // This phone's device key: the PKCS#8 key lives in the Keychain and is handed to the Rust core per call; it never
    // reaches JavaScript. `deviceKey` makes it on first use and answers its public key and id.
    Function("deviceKey") { () throws -> [String: String] in
      let pkcs8 = try DeviceKeychain.pkcs8()
      let info = try CuaCoreJSON.call("device.info", ["pkcs8": pkcs8])
      return ["publicKey": info["publicKey"] as? String ?? "", "id": info["id"] as? String ?? ""]
    }
    Function("deviceSign") { (message: String) throws -> String in
      let out = try CuaCoreJSON.call("device.sign", ["pkcs8": try DeviceKeychain.pkcs8(), "message": message])
      return out["signature"] as? String ?? ""
    }
    // The sealed store: what the app keeps on disk between launches (the outbox, cached threads) is AES-256-GCM sealed
    // with a key that lives in the Keychain on this phone only; JavaScript only ever holds sealed text.
    Function("storeSeal") { (plaintext: String, aad: String) throws -> String in
      let out = try CuaCoreJSON.call("store.seal", ["key": try StoreKeychain.key(), "plaintext": plaintext, "aad": aad])
      return out["sealed"] as? String ?? ""
    }
    Function("storeOpen") { (sealed: String, aad: String) throws -> String in
      let out = try CuaCoreJSON.call("store.open", ["key": try StoreKeychain.key(), "sealed": sealed, "aad": aad])
      return out["plaintext"] as? String ?? ""
    }
    // Sign-out: the key is dropped, so whatever is left sealed on disk can never be opened again.
    Function("storeReset") { () in StoreKeychain.reset() }
    Function("methods") { () -> String in CuaCoreMethods() }
    Function("flows") { () -> String in CuaCoreFlows() }
  }
}

struct CoreError: Error, CustomStringConvertible {
  let description: String
  init(_ d: String) { description = d }
}

enum CuaCoreJSON {
  static func call(_ method: String, _ args: [String: Any]) throws -> [String: Any] {
    let data = try JSONSerialization.data(withJSONObject: args)
    var err: NSString?
    guard let out = CuaCoreCall(method, String(data: data, encoding: .utf8) ?? "{}", &err) else {
      throw CoreError(err as String? ?? "core error")
    }
    return (try JSONSerialization.jsonObject(with: Data(out.utf8)) as? [String: Any]) ?? [:]
  }
}

/// The device key in the Keychain (this phone only, readable while unlocked).
enum DeviceKeychain {
  private static let service = "ai.cua.device-key.v2"
  private static let account = "device"

  static func pkcs8() throws -> String {
    if let k = load() { return k }
    let made = try CuaCoreJSON.call("device.generate", [:])
    guard let key = made["pkcs8"] as? String else { throw CoreError("could not make a device key") }
    let add: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: account,
      kSecValueData as String: Data(key.utf8),
      kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
    ]
    let status = SecItemAdd(add as CFDictionary, nil)
    guard status == errSecSuccess else { throw CoreError("the Keychain refused the device key (\(status))") }
    return key
  }

  private static func load() -> String? {
    let q: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: account,
      kSecReturnData as String: true,
      kSecMatchLimit as String: kSecMatchLimitOne,
    ]
    var item: CFTypeRef?
    guard SecItemCopyMatching(q as CFDictionary, &item) == errSecSuccess, let d = item as? Data else { return nil }
    return String(data: d, encoding: .utf8)
  }
}

/// The store key in the Keychain: this phone only, readable after the first unlock (a notification action that opens
/// the app in the background can still read the outbox).
enum StoreKeychain {
  private static let service = "ai.cua.store-key.v1"
  private static let account = "store"

  static func key() throws -> String {
    if let k = load() { return k }
    let made = try CuaCoreJSON.call("store.generate", [:])
    guard let key = made["key"] as? String else { throw CoreError("could not make a store key") }
    let add: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: account,
      kSecValueData as String: Data(key.utf8),
      kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
    ]
    let status = SecItemAdd(add as CFDictionary, nil)
    guard status == errSecSuccess else { throw CoreError("the Keychain refused the store key (\(status))") }
    return key
  }

  static func reset() {
    let q: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrAccount as String: account]
    SecItemDelete(q as CFDictionary)
  }

  private static func load() -> String? {
    let q: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: account,
      kSecReturnData as String: true,
      kSecMatchLimit as String: kSecMatchLimitOne,
    ]
    var item: CFTypeRef?
    guard SecItemCopyMatching(q as CFDictionary, &item) == errSecSuccess, let d = item as? Data else { return nil }
    return String(data: d, encoding: .utf8)
  }
}
