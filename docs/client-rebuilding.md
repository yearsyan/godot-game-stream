# Rebuilding or relinking mirctl 0.1.0

This archive accompanies `mirctl-0.1.0-macos-arm64.zip`. First-party C source is
in `godot-game-stream-0.1.0-source.zip` from the same release. FFmpeg, SDL3 and
dav1d retain the licenses in `licenses/`; their original source archives are in
`sources/`. Source archive checksums and exact Conan revisions/options are in
`dependency-build.json`. Recipe changes to upstream sources are retained in
`recipes/`, including SDL's source-time CMake edit and FFmpeg's build-time edits.

## Rebuild from source

Use macOS arm64, Xcode 26 with Apple Clang 17, Python 3.11+, Conan 2.28.1,
CMake and the profile in `macos-arm64.profile`. Extract the first-party source
and this archive, then run from the source repository:

```sh
export CONAN_HOME="$PWD/build/conan-rebuild"
conan profile detect
cp /path/to/mirctl-dependencies/macos-arm64.profile "$CONAN_HOME/profiles/default"
python3 clients/mirctl/scripts/build.py --lockfile /path/to/mirctl-dependencies/conan.lock
ctest --test-dir clients/mirctl/build/app/Release --output-on-failure
```

The lockfile selects the original recipe revisions; the build script applies
the recorded feature options. Build tools can be downloaded by Conan. Original
dependency source archives are included for inspection and modification even
when upstream download URLs later change. To use a modified dependency, copy its
included recipe, adapt its `source()` method to your modified local source,
create a Conan package with the matching settings/options, and build the client
against that recipe revision. Update the lockfile to select your modified recipe.
The example uses a separate Conan home to preserve your existing setup.

## Relink existing application objects

The `objects/` directory contains the original application's object files and
`libmirctl_core.a`; `static-libraries/` contains the exact dependency archives and
headers used for the release. `original-link-command.txt` records the link step
with dependency paths replaced by `$DEPENDENCIES/static-libraries/<name>`.

Recreate the `CMakeFiles/` layout from `objects/`, place `libmirctl_core.a` in the
working directory and adapt the command to your local directories and Xcode SDK.
You can replace any dependency archive with an ABI-compatible modified build
before linking. A newly linked macOS executable may need a new ad-hoc signature:

```sh
codesign --force --sign - ./mirctl
```

No release-specific signing key is required to rebuild or relink the client.
