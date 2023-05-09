# MacOS packaging and distribution

We distribute the application as a zipped [MacOS app bundle](https://developer.apple.com/library/archive/documentation/CoreFoundation/Conceptual/CFBundles/BundleTypes/BundleTypes.html#//apple_ref/doc/uid/10000123i-CH101-SW5).

## Notes on codesigning and notarization

Running a binary on a Mac that was not both codesigned **and** notarized by Apple is a pain. The
user needs to run it. Get an error message. Go to System preferences > Security > authorize the app.
Then try again, and finally be presented a button to open the app.

In order to avoid that, we've started distributing codesigned binaries starting from version 1.0.
This is the notes i've taken describing the stepped involved in codesigning the produced macOS
binary on a Linux machine, for posterity. This is not cleaned up.

### Bulk notes from the codesigning experiment

Create an account at https://developer.apple.com.

Pay to get into the developer program. Going the organization way is cumbersome. Go the personal
way. They'll ask for a KYC (gov ID). Wait to be accepted.

Go to "certificates, ids and profiles". Create a new certificate. Select a Developer ID application
certificate to distribute apps outside of the store.

(We should look into the installer feature later on. Maybe we could bundle a bitcoind there.)

They ask for a "Certificate Signing Request (CSR)" that you need to generate on your Mac. I don't
have a Mac. Generate it using OpenSSL:
```
openssl genrsa -out wizardsardine_liana.key 2048
openssl req -new -sha256 -key wizardsardine_liana.key -out wizardsardine_liana_codesigning.csr -subj "/emailAddress=antoine@wizardsardine.com, CN=Antoine Poinsot, C=FR"
```
(Note you have no choice in the size or type of the key here, they expect a RSA(2048) key.)

For the profile type select "G2 Sub-CA". We are using an Xcode newer than 11.4.1 and the codesigning
tool we use supports the new CA.

Now you get to be able to download your certificate (I've stored it as
"antoine_devid_liana_codesigning.cer"). Thankfully `rcodesign` supports various certificate format,
so we don't even have to convert it to PEM.

Download `rcodesign`:
```
curl -OL https://github.com/indygreg/apple-platform-rs/releases/download/apple-codesign%2F0.22.0/apple-codesign-0.22.0-x86_64-unknown-linux-musl.tar.gz
tar -xzf apple-codesign-0.22.0-x86_64-unknown-linux-musl.tar.gz
./apple-codesign-0.22.0-x86_64-unknown-linux-musl/rcodesign --help
```

Sign the packaged application using the `sign` command (mind `--code-signature-flags for the
necessary hardened runtime):
```
./apple-codesign-0.22.0-x86_64-unknown-linux-musl/rcodesign sign --code-signature-flags runtime --pem-source wizardsardine_liana.key --der-source antoine_devid_liana_codesigning.cer Liana.app
```
You can see the chain of certificates was applied using the `diff-signatures` command against
another bundle. The best way to verify the signature is by using the `codesign` command on a Mac.

Finally, we need to notarize the app. Follow the instructions at
https://gregoryszorc.com/docs/apple-codesign/main/apple_codesign_rcodesign.html#notarizing-and-stapling:
- Create an API key from https://appstoreconnect.apple.com/ (and *not* a key from
  https://developer.apple.com/account/resources/authkeys)
- Download it and encode it into a JSON file using the `encode-app-store-connect-api-key` command
- Use the `notary-submit` command to request notarization

```
./apple-codesign-0.22.0-x86_64-unknown-linux-musl/rcodesign notary-submit --max-wait-seconds 600 --api-key-path ./encoded_appstore_api_key.json --staple Liana.app
```
According to
https://developer.apple.com/documentation/security/notarizing_macos_software_before_distribution/customizing_the_notarization_workflow#3087732
this can take up to a hour. I've experienced more. You can see the status of an existing request
using the `notary-log` command.


-------

Resources:
- https://gist.github.com/jcward/d08b33fc3e6c5f90c18437956e5ccc35
- https://github.com/achow101/signapple
- https://developer.apple.com/library/archive/technotes/tn2206/_index.html#//apple_ref/doc/uid/DTS40007919
- https://gregoryszorc.com/docs/apple-codesign/main/index.html
- https://www.apple.com/certificateauthority/
- https://developer.apple.com/documentation/security/notarizing_macos_software_before_distribution

Resources on packaging an application for MacOS:
- https://developer.apple.com/library/archive/documentation/CoreFoundation/Conceptual/CFBundles/BundleTypes/BundleTypes.html#//apple_ref/doc/uid/10000123i-CH101-SW5
