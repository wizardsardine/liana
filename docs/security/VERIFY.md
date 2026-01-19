# Verifying Coincube Releases

This guide explains how to verify the authenticity and integrity of Coincube releases using GPG signatures and SHA256 checksums.

## Why Verify?

Verifying releases ensures:
- **Authenticity**: The release was created by Coincube Technology LLC
- **Integrity**: The files haven't been tampered with during download

## Quick Verification (Recommended)

For each release, we provide:
- `coincube-X.Y.Z-SHA256SUMS.txt` - SHA256 checksums of all release artifacts
- `coincube-X.Y.Z-SHA256SUMS.txt.asc` - GPG signature of the checksums file

### Step 1: Import the Coincube GPG Public Key (One-Time Setup)

Download the public key from the repository:

```bash
curl -O https://raw.githubusercontent.com/coincubetech/coincube/master/docs/security/coincube-release-public.asc
gpg --import coincube-release-public.asc
```

Expected output should include:
```
gpg: key 67F9701BF0D2DAF4: public key "Coincube Release Signing <releases@coincube.io>" imported
```

### Step 2: Download Release Files

Download the artifact you want to install plus the checksums files:

```bash
# Example for macOS ARM64 version 1.5.0
curl -LO https://github.com/coincubetech/coincube/releases/download/v1.5.0/Coincube-1.5.0-macos-arm64.dmg
curl -LO https://github.com/coincubetech/coincube/releases/download/v1.5.0/coincube-1.5.0-SHA256SUMS.txt
curl -LO https://github.com/coincubetech/coincube/releases/download/v1.5.0/coincube-1.5.0-SHA256SUMS.txt.asc
```

### Step 3: Verify the GPG Signature

```bash
gpg --verify coincube-1.5.0-SHA256SUMS.txt.asc
```

Expected output:
```
gpg: assuming signed data in 'coincube-1.5.0-SHA256SUMS.txt'
gpg: Signature made [DATE]
gpg:                using RSA key 67F9701BF0D2DAF4
gpg: Good signature from "Coincube Release Signing <releases@coincube.io>"
```

⚠️ **Warning**: If you see `BAD signature`, do NOT proceed. The checksums file has been tampered with.

### Step 4: Verify the Artifact Checksum

```bash
sha256sum --check coincube-1.5.0-SHA256SUMS.txt --ignore-missing
```

Expected output:
```
Coincube-1.5.0-macos-arm64.dmg: OK
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
ARCH=arm64  # or x64 for Intel Macs
curl -LO https://github.com/coincubetech/coincube/releases/download/v${VERSION}/Coincube-${VERSION}-macos-${ARCH}.dmg
curl -LO https://github.com/coincubetech/coincube/releases/download/v${VERSION}/coincube-${VERSION}-SHA256SUMS.txt
curl -LO https://github.com/coincubetech/coincube/releases/download/v${VERSION}/coincube-${VERSION}-SHA256SUMS.txt.asc

# Verify
gpg --verify coincube-${VERSION}-SHA256SUMS.txt.asc
shasum -a 256 --check coincube-${VERSION}-SHA256SUMS.txt --ignore-missing
```

### Linux

```bash
# Import key (one-time)
curl -O https://raw.githubusercontent.com/coincubetech/coincube/master/docs/security/coincube-release-public.asc
gpg --import coincube-release-public.asc

# Download files
VERSION=1.5.0
curl -LO https://github.com/coincubetech/coincube/releases/download/v${VERSION}/coincube-x86_64-unknown-linux-gnu.tar.gz
curl -LO https://github.com/coincubetech/coincube/releases/download/v${VERSION}/coincube-${VERSION}-SHA256SUMS.txt
curl -LO https://github.com/coincubetech/coincube/releases/download/v${VERSION}/coincube-${VERSION}-SHA256SUMS.txt.asc

# Verify
gpg --verify coincube-${VERSION}-SHA256SUMS.txt.asc
sha256sum --check coincube-${VERSION}-SHA256SUMS.txt --ignore-missing
```

### Windows (PowerShell)

```powershell
# Import key (one-time)
# Install GPG4Win first: https://gpg4win.org/
Invoke-WebRequest -Uri "https://raw.githubusercontent.com/coincubetech/coincube/master/docs/security/coincube-release-public.asc" -OutFile "coincube-release-public.asc"
gpg --import coincube-release-public.asc

# Download files
$VERSION = "1.5.0"
Invoke-WebRequest -Uri "https://github.com/coincubetech/coincube/releases/download/v$VERSION/coincube-x86_64.msi" -OutFile "coincube-x86_64.msi"
Invoke-WebRequest -Uri "https://github.com/coincubetech/coincube/releases/download/v$VERSION/coincube-$VERSION-SHA256SUMS.txt" -OutFile "coincube-$VERSION-SHA256SUMS.txt"
Invoke-WebRequest -Uri "https://github.com/coincubetech/coincube/releases/download/v$VERSION/coincube-$VERSION-SHA256SUMS.txt.asc" -OutFile "coincube-$VERSION-SHA256SUMS.txt.asc"

# Verify signature
gpg --verify "coincube-$VERSION-SHA256SUMS.txt.asc"

# Verify checksum (manual check)
Get-FileHash -Algorithm SHA256 coincube-x86_64.msi
# Compare output with the hash in coincube-$VERSION-SHA256SUMS.txt
```

## Troubleshooting

### "gpg: command not found"

Install GPG:
- **macOS**: `brew install gnupg`
- **Linux**: `sudo apt-get install gnupg` (Debian/Ubuntu) or `sudo yum install gnupg` (RHEL/CentOS)
- **Windows**: Download from [GPG4Win](https://gpg4win.org/)

### "WARNING: This key is not certified with a trusted signature"

This is normal on first import. The warning means you haven't explicitly marked the key as trusted. You can verify the key fingerprint matches:

```
67F9 701B F0D2 DAF4
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
| Email | releases@coincube.io |
| Name | Coincube Release Signing |
| Type | RSA 4096 |
| Usage | Sign only |

## Security Notes

- Always verify both the GPG signature AND the checksum
- Download the public key from the official repository or website
- Never skip verification, especially for financial software
- Report any verification failures to security@coincube.io

## Additional Resources

- [GPG Key Rotation Playbook](../docs/security/GPG_KEY_ROTATION.md)
- [Coincube Security Policy](../SECURITY.md)
