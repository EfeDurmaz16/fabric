# Releases

A release is a git tag `v<version>` on `main`. The tag starts `.github/workflows/release.yml`, which builds, checks and publishes everything below. Nothing is published by hand, and nothing in the workflow uses a secret other than the repository's own `GITHUB_TOKEN`.

## What a release contains

| Asset | What it is |
|---|---|
| `fabric-aarch64-apple-darwin.tar.gz` | macOS on Apple silicon |
| `fabric-x86_64-apple-darwin.tar.gz` | macOS on Intel |
| `fabric-x86_64-unknown-linux-gnu.tar.gz` | Linux on x86_64, built on Ubuntu 22.04, so it needs glibc 2.35 or newer |
| `*.tar.gz.sha256`, `sha256.sum` | The sha256 of each archive, and all of them in one file |
| `fabric.cdx.xml` | A CycloneDX SBOM of the `fabric` crate's dependency tree, made by `cargo-cyclonedx` from `Cargo.lock` |
| `fabric.rb` | The Homebrew formula: a url and a sha256 for each archive |
| `source.tar.gz` | The source at the tag |

Each archive holds the `fabric` binary, `README.md` and `LICENSE`. The test doubles that `crates/fabric` also builds are not shipped.

Linux is gnu, not musl: musl's allocator is measurably slower for a daemon that hashes and copies snapshots, and the time budget is measured on gnu.

## What is signed and what is attested

Nothing is signed. There is no Apple Developer ID, so the macOS binaries are not code signed or notarized; on Apple silicon they carry only the ad hoc signature the linker adds, which lets them run but says nothing about who built them. Gatekeeper and the one command that clears a browser's quarantine are in the [README](https://github.com/EfeDurmaz16/fabric#install).

What is attested is provenance. Each archive gets a GitHub artifact attestation when it is built: a SLSA build provenance statement, signed through Sigstore with a short-lived certificate for this repository's `release.yml` run, and logged in the public Rekor transparency log. It ties the archive's sha256 to the commit, the tag and the workflow that built it. Check one with the GitHub CLI:

```sh
gh attestation verify fabric-aarch64-apple-darwin.tar.gz -R EfeDurmaz16/fabric \
  --signer-workflow EfeDurmaz16/fabric/.github/workflows/release.yml
```

The attestation proves where an archive was built, not that the code is safe; the SBOM says what went into it.

## How a release is cut

1. On a branch, bump `version` in `[workspace.package]` in `Cargo.toml`, the one place it lives; every crate, `fabric --version` and the tag follow it. Update the version in `docs/cli-contract.md`'s examples if they name it. Merge the PR once its checks pass, including the `Release` workflow, which builds every archive on a pull request too.
2. Tag the merge commit on `main` and push the tag:

   ```sh
   git tag v0.1.0 && git push origin v0.1.0
   ```

3. The workflow runs `plan`, builds the three archives on their own runners (`macos-15`, `macos-15-intel`, `ubuntu-22.04`) and attests them, builds the formula, the checksums and the SBOM, then runs `release-smoke.yml`: each archive is downloaded to a clean runner that did not build it and `scripts/release-smoke.sh` runs the checksum, `--version`, the CLI contract against a node it starts and, on macOS, the quarantine behaviour. Only when every smoke passes does `host` create the GitHub Release with every asset. A version with a suffix, such as `v0.2.0-rc.1`, is published as a prerelease.
4. Within the hour, the tap's `update-formula` workflow pulls `fabric.rb` into `EfeDurmaz16/homebrew-tap` after checking every archive against the checksums, the formula and its attestation, then installs it with `brew` on a clean Mac. To pull it at once, run that workflow by hand. Prereleases are not pulled.

## Changing the pipeline

`dist-workspace.toml` is the configuration; `release.yml` is generated from it by [cargo-dist](https://axodotdev.github.io/cargo-dist/) 0.34.0. Edit the configuration and run `dist generate`; never edit `release.yml` by hand, because its `plan` job fails when the file and the configuration disagree. `dist plan` shows what a tag would publish, and `dist build --artifacts=local --target=<host target>` followed by `scripts/release-smoke.sh target/distrib <host target> fabric` runs the build and the smoke on your machine.

The tap's files are in `packaging/homebrew-tap`; its README says how the formula gets there and why the tap pulls instead of the release pushing. The product name is in two places, `formula` in `dist-workspace.toml` and `FORMULA` in the tap's workflow, so a rename is one commit with the crate name.
