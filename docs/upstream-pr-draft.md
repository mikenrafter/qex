DRAFT. NOT FILED. Nobody has approved a pull request to stephenc/qex. Delete this
file from the branch before any pull request, because it is not part of the change.

---

Title (Conventional Commits, as CONTRIBUTING.md asks):

    feat: claim disk space with --disk, and keep space free with reserve_disk and min_free_disk

Body (it becomes the commit message of the squash):

    A job can now claim disk space: `qex submit --disk 6GB`, with `--disk-path`
    for a filesystem other than the one that holds the directory of the job. qex
    starts the job when the free space of that filesystem, less
    `[system] reserve_disk` and the claims of the jobs that operate, is at least
    the claim. A claim is a promise and not a limit, as for memory.

    Why: on a machine with a small disk, several agents started builds that were
    each correct for the cores and the memory, and the root filesystem fell to
    82MB free twice in one day while qex kept starting jobs. The free space of a
    filesystem is not a budget that qex can divide, because other programs write
    to it, so qex reads it at each pass of the scheduler and compares it with the
    claims. A job with no `--disk` is not held by the claim or by the reserve, so
    nobody sees a change who does not ask for one.

    What it adds:

    - `--disk SIZE` and `--disk-path PATH` on `submit` and `run`, and
      `disk`/`disk_path` in `[resources]` of a job file and a pipeline stage.
    - Admission per filesystem, by `st_dev`. A wait begins `waits for disk` and
      has the class of its cause: the claims of this queue (known end), other
      programs (no known end, the queue continues), or a claim larger than the
      filesystem (refused at the submission).
    - `[system] reserve_disk` (default 1GB) and `[system] min_free_disk`
      (default 0, off). The floor holds every job on a nearly full filesystem
      except a job that claims `--disk 0`, which is how a garbage collection
      still runs.
    - The capability `disk`, so an earlier coordinator refuses nothing in
      silence: the CLI names `--disk` and gives the command that replaces it.
    - A grace for a disk wait that has no end (D13): `qex submit --wait` and
      `qex run` cancel a job that has waited 60 seconds (`--disk-wait-limit`) for
      space that programs outside the queue hold, say what to free, and exit
      with the new code 101. This part can be a separate pull request.
    - `disk` and `disk_path` in the records, `qex status`, `qex info`
      (`disks` in the JSON), the JSON schemas, `qex help disk` and the agent
      page. Records of earlier versions read as a job with no claim.

    What it does not do, and why, is on docs/disk.md (N1 to N6): no learned disk
    claims, no cleanup command, no claims across users, no credit for bytes that
    a job already wrote, no enforcement, and no protection from a mount that
    hangs.

Notes for the reviewer (a comment on the pull request, not the body):

- Tests: unit tests for the arithmetic with fake filesystems (`disk::tests`,
  `sched::disk_tests`), and end-to-end tests that use the real filesystem of the
  test directory (`tests/e2e.rs`, "Disk claims"). The design was also run by hand
  on loop-mounted ext4 and on tmpfs, including a coordinator of 0.33.0 beside the
  new CLI and the new coordinator beside a 0.33.0 CLI.
- The one rule that a reviewer may want to change is D3: a claim counts in full
  until the job ends, so the bytes that the job already wrote count two times.
  That errs toward waiting. A credit needs a measure of what each job wrote.
