# Releasing

1. Set the version in the workspace `Cargo.toml` (`[workspace.package] version`) and write `docs/releases/<version>.md`.
2. Commit, then tag and push:

   ```bash
   git tag v0.2.0
   ```

   ```bash
   git push origin v0.2.0
   ```

3. The *Release* workflow tests, builds with the `dist` profile, signs the executable and publishes the GitHub release with:
   - `Diskie-<version>-windows-x64.exe`
   - `Diskie-<version>-windows-x64.exe.minisig`
   - `SHA256SUMS`
   - `latest.json`, which installed copies read to find updates

Publish full releases, not pre-releases. Installed copies look at `/releases/latest/download/latest.json`, and GitHub's "latest" never points at a pre-release.

## The signing key

Updates are only installed if their minisign signature verifies against `keys/diskie-release.pub`, which is compiled into Diskie. The secret key:

- is the `DISKIE_SIGNING_KEY` Actions secret (the full contents of `diskie-release.key`);
- has an offline backup kept by the maintainer. **If it is lost, installed copies can no longer verify updates**, and everyone would have to download a new build by hand once.

To sign by hand instead of in CI:

```bash
DISKIE_SIGNING_KEY_FILE=path/to/diskie-release.key cargo xtask release 0.2.0 target/dist/Diskie.exe dist docs/releases/0.2.0.md
```
