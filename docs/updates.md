# Updating PixMinima on macOS

PixMinima checks the latest stable release of the official GitHub project. By default the Mac app checks every 24 hours while running. Automatic downloading is off at first launch. Automatic checks and downloads can be changed in Settings. The app offers a manual check and waits for an explicit restart. A batch already processing images can finish before installation.

Only the Mac app uses the updater. Image compression and its verification run locally. The network request to GitHub uses the public releases endpoint, without an account, image data, file paths or a unique installation identifier. GitHub receives the ordinary IP address and a generic PixMinima updater user agent. [GitHub Releases API](https://docs.github.com/en/rest/releases/releases#get-the-latest-release)

## Verifying and installing a release

1. The updater reads the release tag and the Apple silicon package name. It accepts a newer stable version with a published file size and SHA-256 digest.
2. It downloads from the official version-specific HTTPS URL into a private temporary directory. It compares the length and SHA-256, then asks macOS to verify the signed disk image and assess it under Gatekeeper policy.
3. It repeats the size, digest, publisher and disk policy checks before mounting the image read-only. Before replacing any application, it checks the publisher’s Developer ID certificate, PixMinima’s exact bundle identifier, version, architecture and Gatekeeper assessment. It repeats the application checks on a staged copy beside the installed application.
4. The installer exchanges the verified application and the current application in one filesystem operation, then asks macOS to launch the new version. A failed launch request restores the previous application. When the installation folder is read-only, the user can install the release from its disk image in a writable Applications folder. The updater never requests administrator credentials.

A future release must update the version in Cargo and the Mac bundle, sign and notarize the app and disk image, and attach the final disk image under the stable download name. The signed disk identifier embeds the version. GitHub generates the release asset digest. Releases without the expected identifier, signature or release asset cannot install from this updater. [Package script](../scripts/package-macos.sh) · [Apple notarization](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)

The test suite checks stable-version ordering, malformed release metadata, correct platform selection and the atomic filesystem exchange. An additional macOS integration test uses a signed older application and a signed, notarized newer disk image. It installs in a temporary folder, checks rollback on a simulated launch error and rejects a repeat installation.

The published 0.1.0 updater was also exercised against the public 0.1.1 release. It fetched the signed disk image from GitHub, verified it, installed it in a private test location and launched the updated app. A second check reported the installation up to date, with all seven tested preferences preserved.
