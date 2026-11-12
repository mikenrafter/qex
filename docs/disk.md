---
title: Disk claims
description: How a job claims disk space with --disk, how qex compares the claims with the free space of each filesystem, and what the reserve and the floor do.
---

*[Home](index.md) · [Agents](agents.md) · [Reference](reference.md) · [Design](design.md) · [Security](security.md) · [Sandbox](sandbox.md)*

# Disk claims

A job can claim disk space in the same way as it claims cores and memory:

```sh
qex submit --wait --cpu 2 --mem 4GB --disk 6GB -- cargo build --release
qex submit --wait --disk 20GB --disk-path /var/cache/build -- ./fetch-everything
qex submit --wait --disk 0 --lock nix-gc -- nix-store --gc
```

**A claim is a promise, and it is not a limit.** qex stops no job that writes
more than it claimed. The claim decides what STARTS and when. A job that claims
6GB and writes 60GB still fills the disk.

The reason for the claim is a fault that qex could not see. Several agents
started builds on a machine with a small disk, and each build was correct for
the cores and the memory. The root filesystem fell to 82MB free, twice in one
day, while qex kept starting jobs. A disk has no budget that qex can divide,
because other programs write to it too. qex therefore reads the free space of
the filesystem and compares it with the claims.

This page lists the rules as claims D1 to D13. The tests name the claim that
they hold. The last section lists what qex does NOT do.

## The rules

**D1. `--disk SIZE` is the claim.** The size is a number with a unit: `3GB`,
`512MB`, `0`. One unit step is 1024, as for `--mem`. The words `half`,
`guess`, `full` and `max` are an error, because the disk has no budget to
divide. A job with no `--disk` has no claim, and every rule below that needs a
claim leaves it alone. A user who never gives `--disk` sees no change, except
for the line in `qex info`.

**D2. `--disk-path PATH` names the filesystem.** The claim is on the filesystem
that holds PATH. The default is the directory of the job (`--cwd`). A relative
path starts in the directory of the job and not in the directory of the shell.
qex removes `.` and `..` with no access to the disk. The path may not exist yet,
because a build writes to `target/` before the directory is there: the nearest
directory above it that exists gives the filesystem. Two paths are on one
filesystem when `st_dev` is the same, whatever their names are. Two filesystems
are separate: a claim on one never holds a job on the other.

**D3. Admission compares the claims with the free space.** A job with a claim
starts when

```
free space of the filesystem  >=  reserve_disk
                                   + the claims of the jobs that operate now on it
                                   + the claim of this job
```

qex reads the free space again in each pass of the scheduler (about twice each
second) and counts it as a program without privilege sees it, so the part that
the system keeps for the superuser is not free. A claim counts in FULL from the
start of the job to its end. The bytes that the job already wrote are in the
free space also, so qex counts them two times, and it errs toward waiting. That
is the safe direction for a disk. A claim on a path that qex cannot read waits,
and the reason names the path: a rule that the user asked for is applied or
refused, and never skipped.

**D4. `[system] reserve_disk` is the space to keep free after the claims.** The
default is `1GB`. It applies to a job that has a claim. A job with no claim is
not held by it. The key is like `reserve_mem`, and it takes the same forms:
`"2GB"` or a number of bytes.

**D5. A claim that can never fit is an error at the submission.** When the claim
and the reserve together are more than the SIZE of the filesystem, no free space
can satisfy the job, so `qex submit` refuses it and says "can never start", with
the three ways out: a smaller `--disk`, a smaller `reserve_disk`, or another
`--disk-path`. No job record exists afterwards. A claim that is larger than the
free space NOW and smaller than the filesystem is no error: it waits. A config
that changes after the submission can leave a job that can never start. That job
stays in the queue as `parked`, and the queue continues behind it.

**D6. `--disk 0` is a decision.** The job holds no space, and no disk test holds
it, not the reserve and not the floor (D8). Use it for a job that frees space,
such as a garbage collection, or that writes nothing that counts. `--mem 0`
is an error, because a claim of no memory lets qex start any number of jobs
together. A claim of no disk is another kind of statement, and the floor needs
a way out: without one, a full disk would also stop the job that cleans it.

**D7. The disk wait has a class, as every wait has.** The reason of the job
begins with `waits for disk`.

| Cause | Class (`qex info`) | What the queue does |
| --- | --- | --- |
| The jobs of this queue hold the space: their claims, or the files they wrote, would close the gap if they stopped | `waits-for-capacity` | The job may keep its place at the front, as for memory. Those jobs release the space when they stop. |
| The space is short, and it would stay short if every job of this queue released its whole claim: other programs, or the files of earlier jobs, hold it | `waits-for-machine` | The queue continues behind the job. The wait has no known end, because qex does not control those programs. |
| The claim and the reserve are more than the filesystem | `parked` | The job never starts. The queue continues behind it. |

The disk test comes before the test of the cores and the memory, and it applies
to a job that is larger than the budget also. A job that qex starts alone must
not start on a full disk.

**D8. `[system] min_free_disk` is a floor.** When the free space of a filesystem
is below the floor, qex starts no job there, whether the job has a claim or not.
The filesystem of a job with no claim is the one that holds its directory. The
default is `0`, and that turns the floor off, so the default changes nothing for
a job with no claim. A job with `--disk 0` is exempt (D6). If qex cannot read the
directory of a job with no claim, the job passes: the floor is a protection and
must not stop a job for a fault of its own measurement. The reason that the
floor gives names the key and the way out.

```toml
[system]
reserve_disk  = "2GB"   # keep this free after the claims of the jobs
min_free_disk = "3GB"   # start nothing on a filesystem with less free than this
```

**D9. The job file and a pipeline stage take the claim.**

```toml
[resources]
cpu       = 2
mem       = "4GB"
disk      = "6GB"
disk_path = "target"
```

The command line replaces the job file. `qex pipeline` stages use the same
`[jobs.resources]` table.

**D10. The claim needs the capability `disk`.** A coordinator of 0.33.0 or
earlier ignores a field that it does not know, gives a job id, and starts the job
with no disk rule. The CLI therefore asks the coordinator what it can do, and it
refuses `--disk` against a coordinator that has no `disk`, with the command that
replaces that coordinator. A job with no `--disk` needs nothing new. A CLI of
0.33.0 works with a coordinator of this build, and a CLI of this build works with
a coordinator of 0.33.0 for every job that has no `--disk`.

**The two config keys are the exception.** qex 0.33.0 refuses a config file with a
key that it does not know, so a file with `reserve_disk` or `min_free_disk` stops
every command of a 0.33.0 program that reads it, and a 0.33.0 coordinator that is
still running keeps its old values and reports the fault. Write the keys only
after the coordinator is this build (`qex info` gives its version). The defaults
need no key.

**D11. The record carries the claim.** `spec.json` and `status.json` have the
fields `disk` (bytes, or `null`) and `disk_path` (an absolute path, or `null`).
`qex status` has a line `disk:`. A record that 0.33.0 wrote has neither field,
and it reads as a job with no claim: a job that operates across the upgrade holds
no space. The JSON schemas name both fields (`qex schema job`, `qex schema
status`, `qex schema pipeline`).

**D12. `qex info` shows each filesystem that matters.** The filesystem that holds
the records of qex is always there, because it is the first to fill. Each
filesystem with a claim of a job that operates or waits is there also. Each line
gives the free space, the size, the claimed bytes and the number of jobs that
hold them, the number of jobs that wait for a claim, the reserve and the floor.
`qex info --json` has the list as `disks`. An earlier coordinator sends no list,
and the CLI prints `unknown`, never a value that it did not measure.

**D13. A wait for disk that has no known end becomes an error, after a grace.**
A command that submits a job and waits for it (`qex submit --wait`, `qex run`,
`qex submit --follow`) stops its wait when the job has been in the class
`waits-for-machine` FOR DISK for 60 seconds without a break. The space is then
held outside the queue: no job of the queue would free it, so the wait has no
known end, and an agent that waits on would wait until its own timeout. The
command instead

1. cancels the job (it did not run, and it wrote no output),
2. writes a message on stderr, and
3. exits with the code **101**.

*What counts.* The three causes of the class `waits-for-machine` for disk: a
claim and its reserve that the free space does not hold, with no claim of the
queue to release (D3, D4); a filesystem below `min_free_disk` (D8); a
`--disk-path` that qex cannot read (D3). The coordinator writes the cause to the
field `blocked_disk` of the record while the cause holds, and clears it when the
cause ends. A wait of the class `waits-for-capacity` (the jobs of this queue hold
the space) never counts, and neither does `parked` (D5), a wait for memory, a
lock, a dependency or a pause: those are waits that have an end, or another
remedy.

*Without a break.* The clock starts when the waiting command first sees
`blocked_disk`, and it starts again from zero each time the field goes. A job
whose wait changes to `waits-for-capacity` and back thus has its 60 seconds
again. The command reads the record once a second while the field is there, so
the stop comes 60 to 62 seconds after the start of the clock.

*The message* names the filesystem and the path, the free space now, what the job
needs (the claim and the reserve, or the floor) and how much to free. It gives
the next steps: `qex submit --wait --cpu 1 --mem 200MB --disk 0 --lock nix-gc --
nix-store --gc` (only when `nix-store` is on the PATH), `du -xsh` on the usual
places, a smaller `--disk`, another `--disk-path`, that a job with `--disk 0` is
exempt (D6), and the option that waits longer.

*Cancel, and not leave queued.* A job that stays in the queue starts at a moment
that nobody chose, once the space comes back, and an agent that fixed the disk and
submitted the job again would then run the work two times. A cancelled job also
frees its dedupe key (`--dedupe-key`), so the second submission is a new job and
not an answer that names the old one. The exception is a job that this command did
not start (a dedupe key gave it): another command owns it, so this command stops
its wait with the code 101, says so, and leaves the job in the queue.
If the job starts in the moment before the cancel, the coordinator refuses the
cancel, and the command waits for the job as before.

*The limit.* `--disk-wait-limit TIME` sets the grace for one call: `30s`, `5m`.
The default is `60s`. The values `0`, `none`, `never` and `unlimited` wait for ever,
as before this claim. There is **no config key** and no environment variable: a
qex of 0.33.0 refuses a config file with a key that it does not know (D10), so a
key would stop every program of that version that reads the file. The option
belongs to the call that waits, and it is not a property of the job, so it is not
in the job file and not in `spec.json`.

*Where it does not apply.* A `qex submit` with no `--wait` queues the job as
before: a person may choose to wait. `qex wait`, `qex status --wait` and
`qex status --follow` attach to a job that another command made, and they never
cancel a job that they did not make, so they wait as before.

*The code.* 101 is a code of the band 97 to 127 (`qex help exit-codes`). It says:
the disk is the cause, no job ran, and the correction is on the disk.
It is not 123 (the job gave up in the queue by its own `--max-queue-time`), not 124
(the reader set a limit) and not 125 (something stopped a job that ran).

*Compatibility (D10).* The capability `disk-wait` says that the coordinator writes
`blocked_disk`. A coordinator without it never sets the field, so the command never
stops its wait; the job waits as before. An explicit `--disk-wait-limit` against
such a coordinator is refused, with the command that replaces it, as `--disk` is.

## What qex does not do

These are decisions, and each one has its reason.

**N1. No learned disk claims.** qex learns the memory of a command from its
peak. The disk has no such measurement that is true: the growth of a filesystem
during a job includes every other writer, and the files of a build stay after
the build ends, so the difference between the start and the end is not the peak.
A claim that qex learned from that number would be wrong in both directions. To
measure the files of the job needs a walk of the tree, which costs input and
output on the disk that is short of space. Give `--disk` from the size that you
know, and read `df` after the first run.

**N2. No cleanup hook.** qex does not run `nix-store --gc` or any other command
when the free space is low. A cleanup deletes files, and qex cannot know which
files are safe to delete on a machine. The floor (D8) stops new work and the
exempt job (D6) lets a person or an agent queue the cleanup, with a lock:
`qex submit --disk 0 --lock nix-gc -- nix-store --gc`.

**N3. No claims across users.** The coordinators of the other users of a machine
share the cores and the memory through the peer files. The disk claims of other
users are not in those files, so qex counts the claims of its own queue only. The
bytes that other users WRITE are in the free space, which qex reads.

**N4. No credit for the bytes that a job already wrote.** See D3. A credit needs
a measure of what each job wrote, and N1 says why that is not available. The cost
is that a job that wrote most of its claim holds that claim two times, in the
claim and in the free space. A claim should therefore be the growth that the
job adds, and not a margin on top of a margin.

**N5. No enforcement.** A claim is a promise. A job with a quota or a `ulimit`
that you set yourself is limited. A job that qex started is not.

**N6. No protection from a filesystem that hangs.** qex reads the free space with
one system call for each path in each pass, and the coordinator makes that call
while it holds its queue. A network mount whose server does not answer can hold
that call for ever, and the queue stops with it, as any program that reads that
mount would stop. Do not give `--disk-path` on a network mount that can go away.
The records and the claims of a local disk have no such risk.

## Checked on real mounts

The unit tests set the free space of filesystems that do not exist. The end-to-end
tests use the real filesystem of the test directory, and `/dev/shm` for two
filesystems. The design was also run on a small loop filesystem and on a `tmpfs`
mount, where a job really filled the space. The commands and the output are in
the report of the change.
