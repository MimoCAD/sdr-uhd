# sdr-uhd

A USRP B2xx driver (B200, B210, B205mini, B206mini) for Rust: a thin
safe wrapper over libuhd's C API (UHD, the USRP Hardware Driver; USRP,
the Universal Software Radio Peripheral).

- `Usrp::open("type=b200")` (empty: the first found); `find()` lists
  attached radios without opening any.
- Receive and transmit rate, frequency (`sdr_core::TuneRequest`), gain,
  antenna and bandwidth.
- `rx_streamer` receives sc16 samples (`sdr_core::Iq`) with the device's
  timestamps, sc16, sc12 or sc8 on the wire; it implements
  `sdr_core::RxSource`. `tx_streamer` transmits them.
- `link_speed` reads the negotiated USB speed from the OS (sysfs on
  Linux, the IOKit registry on macOS) — UHD's C API does not expose it.

```rust
use sdr_core::{Iq, OtwFormat, TuneRequest};
use sdr_uhd::Usrp;

let mut usrp = Usrp::open("type=b200")?;
usrp.set_rx_rate(4e6, 0)?;
usrp.set_rx_freq(&TuneRequest::auto(446.0e6), 0)?;
usrp.set_rx_gain(40.0, 0)?;
let mut rx = usrp.rx_streamer(0, "", OtwFormat::Sc16)?;
let mut buf = vec![Iq::default(); rx.max_num_samps()];
rx.start_continuous()?;
let info = rx.recv_into(&mut buf, 1.0)?;
```

libuhd is found at build time under `$SDRPREFIX` (default
`~/.sdr/prefix`); `build.rs` generates the bindings with bindgen (it needs
libclang: on macOS the Command Line Tools'; elsewhere `libclang-dev`) and
links `-luhd`. The rest of this file builds that prefix from source.

License: MIT OR Apache-2.0 — this crate's own source, which contains no
UHD code (the bindings are generated on your machine from your UHD's
headers). **libuhd itself is GPL-3.0-or-later** (Ettus Research also
offers a commercial licence): a program linked against it is, in the Free
Software Foundation's reading, a combined work, so a binary built with this
crate and given to anyone else comes under GPLv3 — the whole program's
source, under its terms. Running it yourself conveys nothing. Both of this
crate's licences are GPLv3-compatible.

---

# Building libuhd from source

libuhd, libusb, and Boost build from source into a prefix you choose — no
Homebrew, no pip, no sudo required. Steps 0–8 are the strict recipe;
followed top to bottom in one terminal it works, verified from
factory-fresh on Apple Silicon with 128 GB and with 8 GB of memory. On
the 128 GB machine with unbounded `make -j`, an empty `~/.sdr` to a
probed radio took 3½ minutes (peaking at 64 GB of RAM — low-memory
machines cap parallelism, noted inline).

The from-source build exists because the B206mini needs UHD ≥ 4.9,
newer than package managers reliably carry. On Linux with an older
radio, `apt install libuhd-dev uhd-host libclang-dev`, run
`uhd_images_downloader`, and build with `SDRPREFIX=/usr` (Debian trixie
ships UHD 4.8.0: fine for a B205mini, not a B206mini).

## Tested configuration

| Component | Version | Source |
|---|---|---|
| macOS | 26.5–26.6, Apple Silicon | — |
| Xcode CLT | AppleClang 21.0.0 | `xcode-select --install` |
| CMake | 4.4.2 (portable tarball) | github.com/Kitware/CMake releases |
| libusb | 1.0.30 | github.com/libusb/libusb releases |
| Boost | 1.92.0 | archives.boost.io |
| UHD | v4.11.0.0 (release tag) | github.com/EttusResearch/uhd |
| Hardware | USRP B206mini, FW 8.0, FPGA 7.0 | — |
| Rust | stable, edition 2024, bindgen 0.72 | rustup |

**Important:** the B206mini requires UHD ≥ 4.9; older releases will not
enumerate it. This recipe builds the `v4.11.0.0` release tag.

## Directory layout

The stack lives hidden under `~/.sdr`:

```
~/.sdr/
├── prefix/                          # install prefix — $SDRPREFIX
├── cmake-4.4.2-macos-universal/     # portable CMake, never "installed"
├── libusb-1.0.30/                   # build tree (deletable after install)
├── boost_1_92_0/                    # build tree (deletable after install)
├── mako/  markupsafe/               # build-time codegen only
└── uhd/                             # git clone @ v4.11.0.0
```

---

## Step 0 — Workspace, choose your prefix

```
mkdir ~/.sdr
```

Every library installs into `$SDRPREFIX`; the choice is made once,
here, and nothing downstream cares which you picked.

**Option A — user prefix (default):**

```
export SDRPREFIX=$HOME/.sdr/prefix
```

No `sudo` anywhere, and the whole stack removes with one `rm -rf ~/.sdr`.

**Option B — system prefix (`/usr/local`):**

```
export SDRPREFIX=/usr/local
```

The conventional location; prepend `sudo` to the install commands in
Steps 3, 4, and 6, and run Step 7's script as `sudo -E sh fetch-images.sh`.

**All exports in this guide are session-scoped** — if you close the
terminal mid-procedure, re-run the exports from Steps 0, 2, and 5. See
"Making the environment permanent" at the end.

## Step 1 — Xcode Command Line Tools

```
xcode-select --install
```

Provides clang, git, make, and the system `python3` used for build-time
codegen. Already-installed prints an error saying so — fine, continue.

## Step 2 — Portable CMake

```
cd ~/.sdr
curl -LO https://github.com/Kitware/CMake/releases/download/v4.4.2/cmake-4.4.2-macos-universal.tar.gz
tar xf cmake-4.4.2-macos-universal.tar.gz
export PATH=$HOME/.sdr/cmake-4.4.2-macos-universal/CMake.app/Contents/bin:$PATH
cmake --version
```

> **URL gotcha (every download in this guide):** GitHub's
> `releases/latest/download/` shortcut silently serves a tiny "Not
> Found" page when the asset name doesn't match. Pin the tag and
> sanity-check with `ls -l` — a failed download is single-digit bytes,
> and `tar: Unrecognized archive format` means you extracted an error
> page.

## Step 3 — libusb from source

```
cd ~/.sdr
curl -LO https://github.com/libusb/libusb/releases/download/v1.0.30/libusb-1.0.30.tar.bz2
tar xf libusb-1.0.30.tar.bz2 && cd libusb-1.0.30
./configure --prefix=$SDRPREFIX && make -j && make install
ls $SDRPREFIX/lib/libusb-1.0.dylib $SDRPREFIX/include/libusb-1.0/libusb.h
```

Both paths must exist — UHD's configure is pointed at them explicitly in
Step 6.

## Step 4 — Boost from source

The one ugly dependency; `./b2` is the longest single step.

```
cd ~/.sdr
curl -LO https://archives.boost.io/release/1.92.0/source/boost_1_92_0.tar.bz2
tar xf boost_1_92_0.tar.bz2 && cd boost_1_92_0
./bootstrap.sh --prefix=$SDRPREFIX --with-libraries=chrono,date_time,filesystem,program_options,regex,serialization,thread,atomic
./b2 -j8 install
ls $SDRPREFIX/lib | grep boost
```

> **Boost version-picking:** never an `_rc` tarball, and be wary of
> brand-new finals. 1.92.0 is measured good with UHD 4.11.0.0 on macOS
> and on Debian trixie (configure reports `Boost version: 1.92`, libuhd
> 4.11.0 links, and a B206mini streams on it); 1.91.0 is the known-good
> fallback. `system` is deliberately absent from the list: it went
> header-only and Boost ≥ 1.91 rejects it as a build target — if a
> future Boost rejects another name the same way, remove that name and
> re-run. (`./b2 -j8` completed on an 8 GB machine.)

## Step 5 — Mako (build-time codegen, no pip)

UHD's CMake hard-requires the Mako template module even with the Python
API disabled. Clone it and its one dependency onto `PYTHONPATH`; nothing
installs anywhere:

```
cd ~/.sdr
git clone --depth 1 https://github.com/sqlalchemy/mako.git
git clone --depth 1 https://github.com/pallets/markupsafe.git
export PYTHONPATH=$HOME/.sdr/mako:$HOME/.sdr/markupsafe/src
python3 -c "import mako; print(mako.__version__)"
```

`PYTHONPATH` must remain set through UHD's `cmake` **and** `make` —
codegen runs during both.

## Step 6 — UHD v4.11.0.0, stripped to B200 + C API

```
cd ~/.sdr
git clone https://github.com/EttusResearch/uhd.git
cd uhd && git checkout v4.11.0.0
mkdir host/build && cd host/build
cmake -DCMAKE_INSTALL_PREFIX=$SDRPREFIX -DCMAKE_PREFIX_PATH=$SDRPREFIX -DLIBUSB_INCLUDE_DIRS=$SDRPREFIX/include/libusb-1.0 -DLIBUSB_LIBRARIES=$SDRPREFIX/lib/libusb-1.0.dylib -DENABLE_PYTHON_API=OFF -DENABLE_PYMOD_UTILS=OFF -DENABLE_MANUAL=OFF -DENABLE_DOXYGEN=OFF -DENABLE_TESTS=OFF -DENABLE_EXAMPLES=OFF -DENABLE_MAN_PAGES=OFF -DENABLE_X300=OFF -DENABLE_X400=OFF -DENABLE_N300=OFF -DENABLE_N320=OFF -DENABLE_E320=OFF -DENABLE_E300=OFF -DENABLE_MPMD=OFF -DENABLE_USRP1=OFF -DENABLE_USRP2=OFF -DENABLE_B100=OFF -DENABLE_OCTOCLOCK=OFF -DENABLE_B200=ON -DENABLE_C_API=ON ..
```

**Stop and read the configure summary.** The enabled component list
must be exactly: `LibUHD · LibUHD - C API · Utils · USB · B200`. (A
checked-out tag configures as "a development branch of UHD", its version
`4.11.0.HEAD-0-g…`: expected.)

> **Low-memory machines:** unbounded `make -j` spawns one clang per core
> and UHD's heavy translation units take a GB+ each. On an 8 GB machine
> build with **`make -j 1`** (anything wider runs it out of memory); 16 GB+
> is fine with plain `make -j`.

```
make -j && make install
```

> **4.11 adds Protobuf and gRPC**, both inside `if(ENABLE_MPMD)` (the
> networked USRPs). `-DENABLE_MPMD=OFF` above spares you them; with
> cmake's defaults the configure fails twice (FindProtobuf, FindgRPC)
> without saying a version bump introduced them.

> **Images across a version bump:** the B2xx entries in
> `images/manifest.txt` are identical in 4.10 and 4.11 (same FPGA and
> firmware hashes), so an existing `share/uhd/images` carries over; a
> parallel prefix (`~/.sdr-4.11/`) starts with none. Do not generalise —
> the manifest is UHD's to change.

> **Configure gotchas:** the two `-DLIBUSB_*` overrides are **required**
> (no pkg-config in this stack; without them the finder misses the header
> → `Disabling USB support` → `B200 not met` — if a bad result caches,
> delete and recreate the build dir). A missing-Mako error means
> `PYTHONPATH` isn't set in this shell. Warnings about
> `requests`/`numpy`/`ruamel.yaml` are expected; Step 7 handles the only
> consequence.

## Step 7 — Firmware and FPGA images

The stock downloader needs Python packages; instead `fetch-images.sh`
(in this repository) parses UHD's own manifest and fetches from the
Ettus cache directly, verifying every SHA-256 against it:

```
cd <this repository>
sh fetch-images.sh
```

It takes another manifest as argument 1 (default
`~/.sdr/uhd/images/manifest.txt`). Manual fallback: read the b2xx cache
paths from the manifest, `curl -LO` them into
`$SDRPREFIX/share/uhd/images/`, compare `shasum -a 256`, `unzip -jo`
both.

> **Image gotchas:** the B206mini intentionally uses the **b205mini
> bitstream** (same zip, same SHA — don't hunt for a b206-named file);
> the FX3 firmware `usrp_b200_fw.hex` is shared across the whole family.
> **Nothing is ever flashed** — firmware and FPGA load into volatile
> memory on every enumeration; there is no bricking risk.

## Step 8 — Hardware smoke test

Plug the B206mini directly into a USB-C port (bus-powered — avoid
unpowered hubs; on machines with asymmetric ports only the USB 3 port will
do, and `Operating over USB 3.` in the probe output is the truth-teller).

```
$SDRPREFIX/bin/uhd_find_devices
$SDRPREFIX/bin/uhd_usrp_probe
```

`uhd_find_devices` uploads firmware and lists the device;
`uhd_usrp_probe` loads the FPGA, initializes the AD9364, passes a
register loopback test, and prints the device tree. If
`uhd_find_devices` prints nothing, check System Report → USB: if macOS
didn't enumerate it, the problem is cable/port, not this stack. (A B206
held open by another process reads as 2500:0023 "Unknown" and answers no
request — on Linux, `sudo fuser /dev/bus/usb/…` first.)

Then any crate depending on sdr-uhd builds against the prefix:
`cargo build --release` (with `SDRPREFIX` set, or the `~/.sdr/prefix`
default).

## Linux

Steps 0–8 run unchanged on a Raspberry Pi 5 or any Linux, with the
platform swaps: Step 1 is `apt install build-essential libudev-dev
libclang-dev`; Step 2's tarball is `cmake-4.4.2-linux-aarch64.tar.gz`
(binaries under `bin/`); Steps 3 and 6 name `libusb-1.0.so`; Step 8
needs `LD_LIBRARY_PATH=$SDRPREFIX/lib` for UHD's own utilities and the
udev rule (`install -m 644 $SDRPREFIX/lib/uhd/utils/uhd-usrp.rules
/etc/udev/rules.d/`); and `make -j4`. Steps 2–7 took about 25 minutes on
a Pi 5.

### Linking on Linux

A link argument from a dependency's build script never reaches the
binary that depends on it, so sdr-uhd's rpath covers only its own tests
and examples. It publishes the prefix instead (`links = "uhd"`): a crate
that depends on sdr-uhd directly puts this in its own `build.rs`, and its
binaries then run with no `LD_LIBRARY_PATH`:

```rust
fn main() {
    let Ok(prefix) = std::env::var("DEP_UHD_PREFIX") else { return };
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
        println!("cargo:rustc-link-arg=-Wl,-rpath-link,{prefix}/lib");
    }
}
```

`--disable-new-dtags` makes it a DT_RPATH, which (unlike a DT_RUNPATH)
is also searched for libuhd's own Boost libraries. Without it, or with
UHD in a prefix the system's loader does not search (`/etc/ld.so.conf`),
the binary reads "libuhd.so… not found". An application that reaches
sdr-uhd through another crate (sdr-driver) does not see DEP_UHD_PREFIX;
it passes the same arguments with `RUSTFLAGS="-C
link-arg=-Wl,--disable-new-dtags -C link-arg=-Wl,-rpath,$SDRPREFIX/lib"`,
or installs UHD where the loader looks (`/usr/local/lib` and
`ldconfig`).

## Rust gotchas

If bindgen can't find libclang, set `LIBCLANG_PATH` to the Command Line
Tools' `usr/lib`. Debug builds can't hold real sample rates — always
`--release`. If libuhd is rebuilt, `cargo clean` so bindgen regenerates.

## Making the environment permanent

**Option A:** two `~/.zshrc` lines put the UHD utilities on `PATH` and
keep `SDRPREFIX` for rebuilds:

```
echo 'export SDRPREFIX=$HOME/.sdr/prefix' >> ~/.zshrc
echo 'export PATH=$SDRPREFIX/bin:$PATH' >> ~/.zshrc
```

**Option B:** nothing required (`/usr/local/bin` is already on `PATH`);
persist `SDRPREFIX=/usr/local` anyway for `fetch-images.sh` and
rebuilds.

`PYTHONPATH` and the CMake `PATH` entry are build-time only — needed again
only to rebuild UHD. `build.rs` falls back to `~/.sdr/prefix` when
`SDRPREFIX` is unset, so under Option A `cargo build` works from a
completely bare shell.

## Rebuilding and cleaning up

The build trees and CMake tarball are deletable once `$SDRPREFIX` is
populated; the Mako/MarkupSafe clones are needed only to rebuild UHD.

**Option A:** the entire stack uninstalls with `rm -rf ~/.sdr`.

**Option B:** `sudo make uninstall` from the UHD and libusb build trees
while they exist, or remove `/usr/local/{lib,include,bin,share}`'s
`uhd*`, `libusb-1.0*`, and `libboost_*` entries by enumeration — then
`rm -rf ~/.sdr` as in Option A.
