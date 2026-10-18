#!/usr/bin/env nu
# Creates the stable self-signed code-signing certificate in the login keychain.
# Idempotent: does nothing when the certificate already exists. Never run by tests or `just check`.
#
#   just signing-cert              create it (macOS asks to trust it: authenticate once)
#   just signing-cert --dry-run    print the commands without running any

use signing.nu IDENTITY

# macOS ships LibreSSL here; its PKCS#12 output imports into the keychain, Homebrew's OpenSSL 3 needs -legacy.
const OPENSSL = "/usr/bin/openssl"
const VALID_DAYS = "3650"

# Prints the command (with `secret` masked), then runs it unless `dry_run`.
def step [dry_run: bool, command: string, args: list<string>, secret: string = ""] {
    let line = ($args | each {|a| if ($a | str contains " ") { $"'($a)'" } else { $a } } | str join " ")
    let shown = if $secret == "" { $line } else { $line | str replace --all $secret "***" }
    print $"(ansi dark_gray)$ ($command) ($shown)(ansi reset)"
    if not $dry_run {
        run-external $command ...$args
    }
}

def main [
    --dry-run # Print the commands, run nothing.
] {
    if $nu.os-info.name != "macos" and not $dry_run {
        error make {msg: "signing-cert needs macOS: it uses the `security` tool"}
    }

    let keychain = ($"($env.HOME)/Library/Keychains/login.keychain-db")

    if not $dry_run {
        let found = (do { ^security find-certificate -c $IDENTITY $keychain } | complete)
        if $found.exit_code == 0 {
            print $"'($IDENTITY)' already exists in the login keychain: nothing to do."
            return
        }
    } else {
        print "# skipped when `security find-certificate` finds the certificate"
        step true security [find-certificate -c $IDENTITY $keychain]
    }

    let dir = if $dry_run { "<tmp>" } else { mktemp --directory | path expand }
    let config = ($dir | path join cert.cnf)
    let key = ($dir | path join key.pem)
    let cert = ($dir | path join cert.pem)
    let bundle = ($dir | path join cert.p12)
    # Throwaway: protects the PKCS#12 between two commands, then the directory is deleted.
    let password = if $dry_run { "<random>" } else { random chars --length 32 }

    let config_text = $"[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = ($IDENTITY)
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
"
    print $"# ($config): the OpenSSL config below"
    print $config_text
    if not $dry_run { $config_text | save --force $config }

    let result = (try {
        step $dry_run $OPENSSL [req -x509 -newkey rsa:2048 -nodes -keyout $key -out $cert -days $VALID_DAYS -config $config]
        step $dry_run $OPENSSL [pkcs12 -export -inkey $key -in $cert -name $IDENTITY -out $bundle -passout $"pass:($password)"] $password
        step $dry_run security [import $bundle -k $keychain -P $password -T /usr/bin/codesign] $password
        step $dry_run security [add-trusted-cert -p codeSign -k $keychain $cert]
        {ok: true}
    } catch {|e| {ok: false, msg: $e.msg}})

    if not $dry_run { rm --recursive --force $dir }
    if not $result.ok { error make {msg: $"certificate creation failed: ($result.msg)"} }

    step $dry_run security [find-identity -p codesigning $keychain]
    print $"Done. Sign releases with APPLE_SIGNING_IDENTITY=\"($IDENTITY)\"."
}
