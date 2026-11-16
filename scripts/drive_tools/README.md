# RAM filesystem companion

`../build_drive_tools.py` builds the standalone e2fsprogs 1.47.4 `fuse2fs`
server. Its fixed-size ext4 image lives on verified tmpfs; the removable source
partition may still be FAT32, exFAT, NTFS or another supported ordinary filesystem.
The image supplies byte and inode limits for every client, including external editors.

The build fetches the official kernel.org release and checks the SHA-256 published
in that release's `sha256sums.asc`. `--source-archive` supports a previously downloaded
archive. Build dependencies include a C toolchain, make, pkg-config and libfuse3
development files. Runtime also requires `mke2fs`, `debugfs`, `e2fsck` (e2fsprogs),
libfuse3, fusermount3, libblkid and libuuid. Ordinary Cargo tests do not download or
build native tools. `launch.sh` and full workspace packaging include the companion.

The small GPL-2.0 patch implements `RENAME_NOREPLACE` while holding the server's
existing directory-mutation lock. Other rename flags remain rejected. The original
server rejects all nonzero rename flags, preventing the application's atomic
no-overwrite installations.

Every generated `drive-tools/` directory includes the executable, original source
archive, patch, upstream license notice, source hashes and a rebuild script. The
server is a separate process and is not linked into the Rust application.

To run the native acceptance tests after building a debug companion:

```sh
python3 scripts/build_drive_tools.py --profile debug
cargo test -j 2 -p clip-drives --lib native_ram_quotas -- --ignored
cargo test -j 2 -p clip-drives --lib service::tests -- --ignored --test-threads=1
```
