# memori

A Linux command-line tool that prints one report explaining where your RAM is going.

`free` tells you how much RAM is "used", but not by what. The answer is spread over
several places: `smem` knows about programs, `slabtop` about kernel caches,
`arc_summary` about ZFS, and `/proc/meminfo` about everything else, in kernel jargon.
memori reads all of those sources at once and adds them up so that every byte of RAM
lands in exactly one category. Each category also says how easily its memory comes back.

```
━━ OVERVIEW ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  ███████████████████████████████████▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▒▒▒▒▒▒▒▒▒▒░░░░

  █ In use                 30.5 GiB  49%
  ▓ ZFS ARC                20.0 GiB  32%
  ▒ Cache                   8.2 GiB  13%
  ░ Free                    3.8 GiB   6%

  Held back by the kernel  −586 MiB
  Available                11.4 GiB
  ZFS ARC can shrink by    20.0 GiB
  Total available          31.4 GiB

━━ PROGRAMS ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

      PID  Program          Procs       RAM     Swap
      100  firefox              2   9.0 GiB  461 MiB
  2909719  java                     5.0 GiB  256 MiB
           …1 more                  1.0 GiB   51 MiB
  ──────────────────────────────────────────────────
           All 4 processes         15.0 GiB  768 MiB
```

(This sample is rendered from the synthetic test system in `src/render/tests.rs`; a test
keeps it current.)

The overview puts every byte of RAM into one of four groups: memory in use, the ZFS
ARC (which shrinks when RAM runs low), cache the kernel can drop, and free memory.
"Available" is the kernel's own estimate (`MemAvailable`). It is lower than free plus
cache because the kernel keeps reserves for itself; the "Held back by the kernel" row
shows the difference, so the rows add up.

In the programs table, processes are grouped by executable. PID is the group's root
process, the oldest one whose parent isn't in the group. RAM is the proportional set
size (PSS): each page shared between processes is split evenly among them, so the RAM
column adds up to the total without counting anything twice. Swap is the program's
share of its memory that has been swapped out. Swapped memory isn't in the RAM column;
on zram it still occupies RAM, compressed.

## Usage

```bash
sudo memori
```

memori works without root, but the kernel only lets root read `/proc/slabinfo` and
other users' processes. Without root, the report says what it had to leave out.

`memori --help` (this block is checked against the real output by a test):

```text
Explains where your RAM is going

Usage: memori [OPTIONS]

Options:
  -o, --overview       Where RAM is, and how much is available
  -p, --programs       Programs and their memory
  -z, --zfs            ZFS's disk cache
  -k, --kernel         Kernel memory and slab caches
  -c, --cache          File cache and shared memory
  -s, --swap           Swap devices and zram
  -f, --fragmentation  Free memory fragmentation
  -u, --unexplained    Memory no kernel counter accounts for
  -d, --detailed       Print every section, and break the overview down by category
  -a, --all            List all programs
      --top <N>        How many programs to list [default: 15]
      --slabs <N>      How many slab caches to list [default: 12]
  -P, --per-process    List processes one by one instead of grouping them by program
  -h, --headers        Print titles, column headers and totals (the default)
  -H, --no-headers     Leave out titles, column headers and totals
  -n, --no-help        Leave out explanations and warnings
      --color <COLOR>  When to use colors [default: auto] [possible values: auto, always, never]
      --help           Print help
  -V, --version        Print version

With no section flags, prints the overview and the programs.
Run with sudo to include other users' processes and kernel slab caches.
```

For example, `memori -apH` prints every program and nothing else, one per line.
Warnings (such as "run with sudo") appear only when both headers and help are on.

## Installing

With Nix:

```bash
nix run github:avosirenfal/memori
```

```bash
nix profile install github:avosirenfal/memori
```

The flake also provides `overlays.default` (adds `pkgs.memori`), a dev shell, and
checks that build, test, run clippy and check formatting (`nix flake check`).

With Cargo:

```bash
cargo install --path .
```

## How the categories are computed

The kernel's counters fit together through this identity:

```
MemTotal = MemFree + Buffers + Cached + SReclaimable + "used"
```

`Cached` includes `Shmem` (tmpfs and shared memory). memori splits it into the file
cache, which the kernel can drop, and shared memory, which it can't. It then divides
"used" into these parts:

| Category                | Source                                                     |
| ----------------------- | ---------------------------------------------------------- |
| Programs' own data      | `AnonPages`                                                |
| Kernel objects          | `SUnreclaim`                                               |
| Page tables             | `PageTables` + `SecPageTables`                             |
| Kernel stacks & per-CPU | `KernelStack` + `Percpu`                                   |
| Compressed swap         | `nr_zspages` in `/proc/vmstat`, zram `mm_stat`, `Zswap`    |
| GPU buffers             | amdgpu `mem_info_gtt_used`                                 |
| Huge pages              | `Hugetlb`                                                  |
| ZFS ARC                 | arcstats `size` minus headers, dnodes and dbufs (see below) |
| Unexplained             | whatever is left                                           |

ZFS allocates its bookkeeping (buffer headers, dnodes, dbufs) from ordinary kernel slab
caches, so the slab counters already include it. memori therefore counts only the ARC's
cached data and metadata under "ZFS ARC". Without that correction, the bookkeeping
would be counted twice. Some other overlap is possible, for example small ZFS buffers
that also live in slab. If the counters add up to more than the total, the report says
so instead of showing a negative "unexplained".

memori reads everything from `/proc` and `/sys` directly. It doesn't need `smem`,
`slabtop` or the ZFS tools installed.
