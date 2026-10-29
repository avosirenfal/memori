# memori

A Linux command-line tool that prints one report explaining where your RAM is going.

`free` tells you how much RAM is "used", but not by what. The answer is spread over
several places: `smem` knows about programs, `slabtop` about kernel caches,
`arc_summary` about ZFS, and `/proc/meminfo` about everything else, in kernel jargon.
memori reads all of those sources at once and adds them up so that every byte of RAM
lands in exactly one category. Each category also says how easily its memory comes back.

```
━━ WHERE YOUR RAM IS ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  ███████████████████████████████████▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▒▒▒▒▒▒▒▒▒▒░░░░

  █ In use                                      30.5 GiB   49%  freed only by quitting programs
  ▓ ZFS ARC                                     20.0 GiB   32%  given back when RAM runs low
  ▒ Cache                                        8.2 GiB   13%  dropped instantly when needed
  ░ Free                                         3.8 GiB    6%

  Available                                     11.4 GiB  free + droppable cache (MemAvailable)
  ZFS ARC can shrink by                         20.0 GiB  down to its minimum size
  Total available                               31.4 GiB

━━ PROGRAMS ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  Program                                    Unique     Fair share      Swap
                                      freed if quit   incl. shared
  firefox                    2 procs        7.2 GiB        9.0 GiB   461 MiB
  java                       pid 200        4.0 GiB        5.0 GiB   256 MiB
  …1 more                                   819 MiB        1.0 GiB    51 MiB
  ──────────────────────────────────────────────────────────────────────────
  All 4 processes                          12.0 GiB       15.0 GiB   768 MiB
```

(This sample comes from the synthetic test system in `src/report.rs`.)

That is the default summary. With `--detailed`, every total in the overview is broken
down into its categories, and these sections follow:

- **ZFS ARC**: its size, target, minimum and maximum, what it holds, how much it could
  give back, and the hit rate.
- **Kernel**: slab, page tables, stacks, per-CPU data and vmalloc, followed by the
  largest slab caches (`slabtop -s c`). Each cache gets a plain-language description,
  how full it is, how much space it wastes to fragmentation, and whether the kernel
  can free it.
- **File cache & shared memory**: active and inactive cache, dirty pages, and the tmpfs
  mounts that make up shared memory. `free` counts shared memory as cache even though
  it can't be dropped.
- **Swap**: swap devices and zram compression ratios.
- **Free memory fragmentation**: how much free RAM is in large contiguous blocks,
  from `/proc/buddyinfo`.
- **Unexplained memory**: if a lot of RAM isn't covered by any counter, the measurable
  suspects.

In the programs table, *Unique* (USS) is roughly what you get back by quitting the
program. *Fair share* (PSS) also includes an even split of the memory it shares with
other processes, so fair shares add up without double counting.

## Usage

```bash
sudo memori
```

memori works without root, but the kernel only lets root read `/proc/slabinfo` and
other users' processes. Without root, the report says what it had to leave out.

| Option              | Meaning                                                |
| ------------------- | ------------------------------------------------------ |
| `-d, --detailed`    | break totals down and add a section per data source    |
| `-n, --programs N`  | how many programs to list (default 15)                 |
| `-s, --slabs N`     | how many slab caches to list with `-d` (default 12)    |
| `-p, --per-process` | list individual processes instead of grouping them     |
| `--color WHEN`      | `auto` (default), `always` or `never`; honors NO_COLOR |

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
