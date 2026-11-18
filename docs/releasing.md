# Releasing

1. Set the version in the workspace `Cargo.toml` (`[workspace.package] version`) and write `docs/releases/<version>.md`.
2. Commit, then tag and push:

   ```bash
   git tag v0.2.0
   ```

   ```bash
   git push origin v0.2.0
   ```

3. The *Release* workflow tests and builds Windows (on `windows-latest`) and Linux (on `ubuntu-22.04`, so the binary runs on older glibc too) with the `dist` profile, packages the Linux build, signs every download and publishes the GitHub release with:
   - `Diskie-<version>-windows-x64.exe`
   - `Diskie-<version>-x86_64.AppImage`
   - `Diskie-<version>-linux-x64.tar.gz` (the binary, a menu entry, the icon and `install.sh`)
   - `Diskie-<version>-linux-x64`, the bare Linux binary that tar.gz installs update themselves from
   - a `.minisig` signature for each of those
   - `SHA256SUMS`
   - `latest.json`, which installed copies read to find updates

Publish full releases, not pre-releases. Installed copies look at `/releases/latest/download/latest.json`, and GitHub's "latest" never points at a pre-release.

## The signing key

Updates are only installed if their minisign signature verifies against `keys/diskie-release.pub`, which is compiled into Diskie. The secret key:

- is the `DISKIE_SIGNING_KEY` Actions secret (the full contents of `diskie-release.key`);
- has an offline backup kept by the maintainer. **If it is lost, installed copies can no longer verify updates**, and everyone would have to download a new build by hand once.

To sign by hand instead of in CI (one `<platform>=<file>` per update download, `extra=` for files that are only published):

```bash
DISKIE_SIGNING_KEY_FILE=path/to/diskie-release.key cargo xtask release 0.2.0 dist docs/releases/0.2.0.md windows-x86_64=target/dist/Diskie.exe
```

The Linux packages come from `packaging/linux/package.sh <version> <binary> <out-dir>`; `packaging/linux/build-deps.sh` installs what building on Ubuntu or Debian needs.
