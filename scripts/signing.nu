# Code-signing constants shared by the scripts. `just release` signs with this identity.

# Common name of the self-signed certificate in the login keychain, and the value for
# `APPLE_SIGNING_IDENTITY` (Tauri) and `codesign --sign`. Keychain access-list entries and macOS
# folder grants are tied to the signature, so this name and the certificate must stay stable.
export const IDENTITY = "TokenMtr Local Signing"
