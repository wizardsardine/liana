# Verifying Coincube Releases

This guide explains how to verify the authenticity and integrity of Coincube releases using GPG signatures and SHA256 checksums.

## Why Verify?

Verifying releases ensures:
- **Authenticity**: The release was created by Coincube Technology LLC
- **Integrity**: The files haven't been tampered with during download

## Quick Verification (Recommended)

For each release, we provide:
- `SHA256SUMS-X.Y.Z.txt` - SHA256 checksums of all release artifacts
- `SHA256SUMS-X.Y.Z.txt.asc` - GPG signature of the checksums file

Release artifacts are named `tenshu-<version>-<target>.<ext>`, where `<target>`
is the Rust target triple. For version 1.5.0:

| Platform | Filename |
|----------|----------|
| macOS (Apple Silicon) | `tenshu-1.5.0-aarch64-apple-darwin.dmg` |
| macOS (Intel) | `tenshu-1.5.0-x86_64-apple-darwin.dmg` |
| Windows | `tenshu-1.5.0-x86_64-pc-windows-msvc.msi` |
| Linux | `tenshu-1.5.0-x86_64-unknown-linux-gnu.tar.gz` |

### Step 1: Import the Coincube GPG Public Key (One-Time Setup)

Download the public key from the repository:

```bash
curl -O https://raw.githubusercontent.com/coincubetech/coincube/master/docs/security/coincube-release-public.asc
gpg --import coincube-release-public.asc
```

Expected output should include:
```bash
gpg: key 67F9701BF0D2DAF4: public key "Coincube Release Signing <releases@coincube.io>" imported
```

### Step 2: Download Release Files

Download the artifact you want to install plus the checksums files:

```bash
# Example for macOS Apple Silicon (arm64), version 1.5.0
curl -LO https://github.com/coincubetech/coincube/releases/download/v1.5.0/tenshu-1.5.0-aarch64-apple-darwin.dmg
curl -LO https://github.com/coincubetech/coincube/releases/download/v1.5.0/SHA256SUMS-1.5.0.txt
curl -LO https://github.com/coincubetech/coincube/releases/download/v1.5.0/SHA256SUMS-1.5.0.txt.asc
```

### Step 3: Verify the GPG Signature

```bash
gpg --verify SHA256SUMS-1.5.0.txt.asc
```

Expected output:
```bash
gpg: assuming signed data in 'SHA256SUMS-1.5.0.txt'
gpg: Signature made [DATE]
gpg:                using RSA key 67F9701BF0D2DAF4
gpg: Good signature from "Coincube Release Signing <releases@coincube.io>"
```

⚠️ **Warning**: If you see `BAD signature`, do NOT proceed. The checksums file has been tampered with.

### Step 4: Verify the Artifact Checksum

```bash
sha256sum --check SHA256SUMS-1.5.0.txt --ignore-missing
```

Expected output:
```bash
tenshu-1.5.0-aarch64-apple-darwin.dmg: OK
```

✅ If both verifications pass, your download is authentic and safe to install.

## Platform-Specific Examples

### macOS

```bash
# Import key (one-time)
curl -O https://raw.githubusercontent.com/coincubetech/coincube/master/docs/security/coincube-release-public.asc
gpg --import coincube-release-public.asc

# Download files
VERSION=1.5.0
TARGET=aarch64-apple-darwin  # or x86_64-apple-darwin for Intel Macs
curl -LO https://github.com/coincubetech/coincube/releases/download/v${VERSION}/tenshu-${VERSION}-${TARGET}.dmg
curl -LO https://github.com/coincubetech/coincube/releases/download/v${VERSION}/SHA256SUMS-${VERSION}.txt
curl -LO https://github.com/coincubetech/coincube/releases/download/v${VERSION}/SHA256SUMS-${VERSION}.txt.asc

# Verify
gpg --verify SHA256SUMS-${VERSION}.txt.asc
shasum -a 256 --check SHA256SUMS-${VERSION}.txt --ignore-missing
```

### Linux

```bash
# Import key (one-time)
curl -O https://raw.githubusercontent.com/coincubetech/coincube/master/docs/security/coincube-release-public.asc
gpg --import coincube-release-public.asc

# Download files
VERSION=1.5.0
curl -LO https://github.com/coincubetech/coincube/releases/download/v${VERSION}/tenshu-${VERSION}-x86_64-unknown-linux-gnu.tar.gz
curl -LO https://github.com/coincubetech/coincube/releases/download/v${VERSION}/SHA256SUMS-${VERSION}.txt
curl -LO https://github.com/coincubetech/coincube/releases/download/v${VERSION}/SHA256SUMS-${VERSION}.txt.asc

# Verify
gpg --verify SHA256SUMS-${VERSION}.txt.asc
sha256sum --check SHA256SUMS-${VERSION}.txt --ignore-missing
```

### Windows (PowerShell)

```powershell
# Import key (one-time)
# Install GPG4Win first: https://gpg4win.org/
Invoke-WebRequest -Uri "https://raw.githubusercontent.com/coincubetech/coincube/master/docs/security/coincube-release-public.asc" -OutFile "coincube-release-public.asc"
gpg --import coincube-release-public.asc

# Download files
$VERSION = "1.5.0"
Invoke-WebRequest -Uri "https://github.com/coincubetech/coincube/releases/download/v$VERSION/tenshu-$VERSION-x86_64-pc-windows-msvc.msi" -OutFile "tenshu-$VERSION-x86_64-pc-windows-msvc.msi"
Invoke-WebRequest -Uri "https://github.com/coincubetech/coincube/releases/download/v$VERSION/SHA256SUMS-$VERSION.txt" -OutFile "SHA256SUMS-$VERSION.txt"
Invoke-WebRequest -Uri "https://github.com/coincubetech/coincube/releases/download/v$VERSION/SHA256SUMS-$VERSION.txt.asc" -OutFile "SHA256SUMS-$VERSION.txt.asc"

# Verify signature
gpg --verify "SHA256SUMS-$VERSION.txt.asc"

# Verify checksum (manual check)
Get-FileHash -Algorithm SHA256 tenshu-$VERSION-x86_64-pc-windows-msvc.msi
# Compare output with the hash in SHA256SUMS-$VERSION.txt
```

## Troubleshooting

### "gpg: command not found"

Install GPG:
- **macOS**: `brew install gnupg`
- **Linux**: `sudo apt-get install gnupg` (Debian/Ubuntu) or `sudo yum install gnupg` (RHEL/CentOS)
- **Windows**: Download from [GPG4Win](https://gpg4win.org/)

### "WARNING: This key is not certified with a trusted signature"

This is normal on first import. The warning means you haven't explicitly marked the key as trusted. You can verify the key fingerprint matches:

```text
ED4D 0D71 0103 D625 2854  6813 67F9 701B F0D2 DAF4
```

To mark as trusted:
```bash
gpg --edit-key releases@coincube.io
> trust
> 5 (ultimate trust)
> quit
```

### "No such file or directory"

Ensure you're in the directory containing the downloaded files, or provide full paths.

## GPG Key Information

| Property | Value |
|----------|-------|
| Key ID | `67F9701BF0D2DAF4` |
| Email | [releases@coincube.io](mailto:releases@coincube.io) |
| Name | Coincube Release Signing |
| Type | RSA 4096 |
| Usage | Sign only |

## Security Notes

- Always verify both the GPG signature AND the checksum
- Download the public key from the official repository or website
- Never skip verification, especially for financial software
- Report any verification failures to [security@coincube.io](mailto:security@coincube.io)

## Additional Resources

- [GPG Key Rotation Playbook](./GPG_KEY_ROTATION.md)
