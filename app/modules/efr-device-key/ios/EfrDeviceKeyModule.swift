// The device key of the efr app: a P-256 key in the Secure Enclave that proves this phone
// to efrd. efrd keeps the public key from the pairing and checks a signature on each
// connection (device.prove).
//
// The private key never leaves the Secure Enclave. The app keeps only the encrypted handle
// that CryptoKit gives (`dataRepresentation`), in the keychain, with
// kSecAttrAccessibleWhenUnlockedThisDeviceOnly: it is not in a backup, it does not go to a
// new phone, and the key signs only while the phone is unlocked. The key has no Face ID
// gate, because the app proves the key on each reconnect.
//
// The Simulator has no Secure Enclave. A Simulator build uses a software key instead, so
// that the pairing flow can run in development. A build for a device has no software path.
//
// Values cross to JavaScript as standard base64 strings: the public key (the 65-byte X9.63
// point, 0x04 then X and Y), the message to sign and the signature (ECDSA with SHA-256, in
// DER).

import CryptoKit
import ExpoModulesCore
import Foundation
import Security

@ExpoModule("EfrDeviceKey")
public class EfrDeviceKeyModule: Module {
  /// True when the key is in the Secure Enclave, false for the software key of the
  /// Simulator.
  @JS
  func isHardwareBacked() -> Bool {
    #if targetEnvironment(simulator)
      return false
    #else
      return SecureEnclave.isAvailable
    #endif
  }

  /// Makes a new key under `alias` in place of any old one, and returns its public key.
  @JS(.concurrent)
  func createKey(alias: String) async throws -> String {
    try DeviceKeychain.checkAlias(alias)
    return try DeviceKeychain.lock.withLock {
      let key = try DeviceKey.make()
      try DeviceKeychain.write(alias: alias, data: key.storedData)
      return key.publicKey.base64EncodedString()
    }
  }

  /// The public key under `alias`, or nil when there is no key.
  @JS(.concurrent)
  func publicKey(alias: String) async throws -> String? {
    try DeviceKeychain.checkAlias(alias)
    return try DeviceKeychain.lock.withLock {
      guard let data = try DeviceKeychain.read(alias: alias) else {
        return nil
      }
      return try DeviceKey.load(data).publicKey.base64EncodedString()
    }
  }

  /// Signs `message` (standard base64) with the key under `alias`. The result is the DER
  /// form of the ECDSA signature over the SHA-256 of the message, as standard base64.
  @JS(.concurrent)
  func sign(alias: String, message: String) async throws -> String {
    try DeviceKeychain.checkAlias(alias)
    guard let bytes = Data(base64Encoded: message) else {
      throw InvalidMessageException()
    }
    return try DeviceKeychain.lock.withLock {
      guard let data = try DeviceKeychain.read(alias: alias) else {
        throw KeyNotFoundException(alias)
      }
      return try DeviceKey.load(data).sign(bytes).base64EncodedString()
    }
  }

  /// Removes the key under `alias`. A missing key is not an error.
  @JS(.concurrent)
  func deleteKey(alias: String) async throws {
    try DeviceKeychain.checkAlias(alias)
    try DeviceKeychain.lock.withLock {
      try DeviceKeychain.remove(alias: alias)
    }
  }
}

/// One device key: in the Secure Enclave on a device, in software in the Simulator.
private enum DeviceKey {
  case enclave(SecureEnclave.P256.Signing.PrivateKey)
  #if targetEnvironment(simulator)
    case software(P256.Signing.PrivateKey)
  #endif

  static func make() throws -> DeviceKey {
    #if targetEnvironment(simulator)
      return .software(P256.Signing.PrivateKey())
    #else
      guard SecureEnclave.isAvailable else {
        throw NoSecureEnclaveException()
      }
      var error: Unmanaged<CFError>?
      guard
        let access = SecAccessControlCreateWithFlags(
          kCFAllocatorDefault,
          kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
          .privateKeyUsage,
          &error
        )
      else {
        throw KeyFailedException("the access control of the key: \(String(describing: error?.takeRetainedValue()))")
      }
      do {
        return .enclave(
          try SecureEnclave.P256.Signing.PrivateKey(compactRepresentable: false, accessControl: access))
      } catch {
        throw KeyFailedException("the Secure Enclave did not make the key: \(error.localizedDescription)")
      }
    #endif
  }

  static func load(_ data: Data) throws -> DeviceKey {
    do {
      #if targetEnvironment(simulator)
        return .software(try P256.Signing.PrivateKey(rawRepresentation: data))
      #else
        return .enclave(try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: data))
      #endif
    } catch {
      throw KeyFailedException("the stored key does not load: \(error.localizedDescription)")
    }
  }

  /// What the keychain keeps: the encrypted handle of a Secure Enclave key, or the raw
  /// private key of the Simulator's software key.
  var storedData: Data {
    switch self {
    case .enclave(let key):
      return key.dataRepresentation
    #if targetEnvironment(simulator)
      case .software(let key):
        return key.rawRepresentation
    #endif
    }
  }

  /// The 65-byte X9.63 public key.
  var publicKey: Data {
    switch self {
    case .enclave(let key):
      return key.publicKey.x963Representation
    #if targetEnvironment(simulator)
      case .software(let key):
        return key.publicKey.x963Representation
    #endif
    }
  }

  /// The DER signature over the SHA-256 of `message`.
  func sign(_ message: Data) throws -> Data {
    do {
      switch self {
      case .enclave(let key):
        return try key.signature(for: message).derRepresentation
      #if targetEnvironment(simulator)
        case .software(let key):
          return try key.signature(for: message).derRepresentation
      #endif
      }
    } catch {
      throw KeyFailedException("the key did not sign: \(error.localizedDescription)")
    }
  }
}

/// The keychain items that hold the device keys, one generic password per alias.
private enum DeviceKeychain {
  static let service = "dev.eggfriedrice.efr.device-key"

  /// One operation at a time, so that two calls for the same alias cannot mix a write
  /// and a read.
  static let lock = NSLock()

  /// An alias is 1 to 64 ASCII letters, digits, `.`, `_` or `-`.
  static func checkAlias(_ alias: String) throws {
    let allowed = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-")
    guard !alias.isEmpty, alias.count <= 64, alias.unicodeScalars.allSatisfy({ allowed.contains($0) }) else {
      throw InvalidAliasException()
    }
  }

  static func baseQuery(alias: String) -> [String: Any] {
    return [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: alias,
    ]
  }

  static func read(alias: String) throws -> Data? {
    var query = baseQuery(alias: alias)
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var item: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &item)
    if status == errSecItemNotFound {
      return nil
    }
    guard status == errSecSuccess, let data = item as? Data else {
      throw KeychainException(status)
    }
    return data
  }

  static func write(alias: String, data: Data) throws {
    try remove(alias: alias)
    var query = baseQuery(alias: alias)
    query[kSecValueData as String] = data
    query[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
    query[kSecAttrSynchronizable as String] = false
    let status = SecItemAdd(query as CFDictionary, nil)
    guard status == errSecSuccess else {
      throw KeychainException(status)
    }
  }

  static func remove(alias: String) throws {
    let status = SecItemDelete(baseQuery(alias: alias) as CFDictionary)
    guard status == errSecSuccess || status == errSecItemNotFound else {
      throw KeychainException(status)
    }
  }
}

private final class NoSecureEnclaveException: Exception {
  override var reason: String {
    "This iPhone has no Secure Enclave, so efr cannot make a device key"
  }
}

private final class KeyNotFoundException: GenericException<String> {
  override var reason: String {
    "There is no device key '\(param)'"
  }
}

private final class KeyFailedException: GenericException<String> {
  override var reason: String {
    "The device key failed: \(param)"
  }
}

private final class KeychainException: GenericException<OSStatus> {
  override var reason: String {
    "The keychain failed with OSStatus \(param)"
  }
}

private final class InvalidAliasException: Exception {
  override var reason: String {
    "A key alias is 1 to 64 ASCII letters, digits, '.', '_' or '-'"
  }
}

private final class InvalidMessageException: Exception {
  override var reason: String {
    "The message to sign is not valid base64"
  }
}
