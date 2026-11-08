# Homebrew tap template

The files of the `EfeDurmaz16/homebrew-tap` repository, kept here so a
rename or a change to the update job is reviewed with the release it
belongs to. To set the tap up, create an empty public repository named
`homebrew-tap` under `EfeDurmaz16` and copy this directory's contents to its
root (`TAP.md` becomes its `README.md`). Nothing else is needed: no token, no
secret.

How the formula gets there: the release workflow of `EfeDurmaz16/fabric`
builds `fabric.rb` as a release asset, with a url and a sha256 for each of
the three archives. `.github/workflows/update-formula.yml` here runs in the
tap every hour and on demand, downloads that formula and the archives of the
newest release, checks each archive against the release's `sha256.sum`, the
formula's sha256 lines and its GitHub artifact attestation, and commits
`Formula/fabric.rb`. A second job installs it with `brew` on a clean macOS
runner and runs `fabric --version`.

The tap pulls instead of the release pushing because a push from
`EfeDurmaz16/fabric` would need a token with write access to another
repository; this way each repository uses only its own `GITHUB_TOKEN`.

The product name is in two places: `formula` in `dist-workspace.toml` and
`FORMULA` at the top of the update workflow. A rename changes both, with
the crate name.

GitHub turns off scheduled workflows in a public repository after 60 days
without a commit. A release commits the formula, which keeps it on; after a
quiet spell, run the workflow by hand from the Actions tab or with
`gh workflow run update-formula.yml -R EfeDurmaz16/homebrew-tap`.
