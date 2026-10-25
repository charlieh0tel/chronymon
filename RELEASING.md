# Releasing

1. Bump the workspace version and the path dependencies' versions;
   `cargo update -w`.  Add a `packaging/debian/changelog` entry.
2. `make ci`, `make test-web`, `cargo audit`.
3. Merge through a PR; wait for CI on `main`.
4. `git tag -a vX.Y.Z -m "chronymon X.Y.Z" && git push origin vX.Y.Z`.

The tag builds amd64 and arm64 `.deb`s, publishes the GitHub Release
and triggers the apt-repo rebuild.  Never re-tag; fix forward.
