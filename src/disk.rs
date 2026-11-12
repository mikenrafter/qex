//! This module decides if a job fits the free space of a filesystem.
//!
//! A job can claim disk space with `--disk SIZE`. The claim is a promise and
//! not a limit, in the same way as a claim of memory: qex stops no job that
//! writes more than it claimed. The claim decides what STARTS and when.
//! Read `docs/disk.md` for the whole design. The numbers D1 to D12 in the
//! comments below are the numbers of that page.
//!
//! # Why the free space is read, and not a budget
//!
//! Memory has a budget, because the memory of a machine is a fixed number that
//! qex can divide. The free space of a filesystem is not such a number: a
//! program that qex does not control writes to the same filesystem, and a
//! package manager keeps the files of a build after the build stops. qex
//! therefore reads the free space of the filesystem at each pass of the
//! scheduler, and it compares that number with the claims (D3).
//!
//! # Why this module holds a pure function
//!
//! [`judge`] reads its inputs and writes nothing. The scheduler measures the
//! filesystems one time for each pass (see [`Disks`]) and gives the numbers
//! here, so a test can set the free space of a filesystem that does not exist.

use crate::sys::Fs;
use crate::units::format_size;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The space that one job claims, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskClaim {
    /// The bytes that the job claims. The value 0 is a decision: the job
    /// claims nothing and no disk test holds it (D6).
    pub bytes: u64,
    /// The path whose filesystem holds the claim. It is absolute.
    pub path: PathBuf,
}

/// The two numbers of the configuration that [`judge`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// The space to keep free after the claims (D4). `[system] reserve_disk`.
    pub reserve: u64,
    /// The free space below which qex starts no job (D8). `[system]
    /// min_free_disk`. The value 0 means that there is no such floor.
    pub floor: u64,
}

/// The answer of [`judge`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Judgement {
    /// The job can start, as far as the disk is concerned.
    Fits,
    /// The claims of the jobs of this queue hold the space that the job needs.
    /// Those jobs release it when they stop, so this wait has a known end.
    Siblings { fact: String },
    /// The space is short, whatever the jobs of this queue do: programs outside
    /// the queue, or the files of earlier jobs, hold it. qex does not control
    /// that, so this wait has no known end.
    Outside { reason: String },
    /// The job can never start: the claim is larger than the filesystem.
    Never { reason: String },
}

/// Reads the filesystem of a path. A test gives its own.
pub type Probe = Box<dyn Fn(&Path) -> Option<Fs>>;

/// The measurements of the filesystems for one pass of the scheduler.
///
/// A path gets ONE measurement for the whole pass, as [`crate::sched`] does for
/// the memory. Two jobs of one pass then see the same number, and the pass does
/// not call the system twice for one path.
pub struct Disks {
    probe: Probe,
    seen: RefCell<BTreeMap<PathBuf, Option<Fs>>>,
}

impl Disks {
    /// Measures the real filesystems.
    pub fn system() -> Self {
        Self::with(Box::new(crate::sys::filesystem))
    }

    pub fn with(probe: Probe) -> Self {
        Self {
            probe,
            seen: RefCell::new(BTreeMap::new()),
        }
    }

    /// Gives filesystems that a test sets. The entry with the longest prefix of
    /// the path answers, as a mount point does.
    #[cfg(test)]
    pub fn fixed(entries: Vec<(&str, Fs)>) -> Self {
        let entries: Vec<(PathBuf, Fs)> = entries
            .into_iter()
            .map(|(p, f)| (PathBuf::from(p), f))
            .collect();
        Self::with(Box::new(move |path| {
            entries
                .iter()
                .filter(|(p, _)| path.starts_with(p))
                .max_by_key(|(p, _)| p.as_os_str().len())
                .map(|(_, f)| *f)
        }))
    }

    /// Gives the filesystem of a path, or `None` when qex cannot read it.
    pub fn stat(&self, path: &Path) -> Option<Fs> {
        if let Some(known) = self.seen.borrow().get(path) {
            return *known;
        }
        let fresh = (self.probe)(path);
        self.seen.borrow_mut().insert(path.to_path_buf(), fresh);
        fresh
    }
}

/// Sums the claims of the jobs that hold space on one filesystem.
pub fn held_on(held: &[DiskClaim], dev: u64, disks: &Disks) -> u64 {
    held.iter()
        .filter(|h| h.bytes > 0)
        .filter(|h| disks.stat(&h.path).map(|f| f.dev) == Some(dev))
        .fold(0u64, |sum, h| sum.saturating_add(h.bytes))
}

/// Tests one job against the free space of its filesystem.
///
/// `claim` is the claim of the job, or `None` for a job that claims nothing.
/// `cwd` is the directory of the job: a job with no claim is tested against
/// the floor on the filesystem of its directory (D8). `held` holds the claims
/// of the jobs that operate now.
///
/// The order of the tests is the order of the claims. The exemption comes
/// first (D6), then the floor (D8), then the size (D5), then the free space
/// without the jobs of this queue (D7, `Outside`), and last the free space with
/// them (D3, `Siblings`). The order decides the class of a wait that two tests
/// would both give: when the space is short even if every job of this queue
/// stopped, the jobs of this queue are not the cause, so the class says
/// `Outside` and not `Siblings`.
pub fn judge(
    rules: Rules,
    claim: Option<&DiskClaim>,
    cwd: &Path,
    held: &[DiskClaim],
    disks: &Disks,
) -> Judgement {
    // D6: a claim of 0 is a decision, and no disk test holds the job.
    if claim.is_some_and(|c| c.bytes == 0) {
        return Judgement::Fits;
    }

    let path = claim.map(|c| c.path.as_path()).unwrap_or(cwd);
    let Some(fs) = disks.stat(path) else {
        // D3. A claim is a rule that the user asked for, so a path that qex
        // cannot read is a wait that says why. The floor is a protection that
        // nobody asked for on this job, so it passes (D8).
        return match claim {
            Some(c) => Judgement::Outside {
                reason: format!(
                    "waits for disk: qex cannot read the free space of {}, so it cannot test \
                     the claim of {}. Check that the path exists and that qex can read it, or \
                     give another --disk-path. qex does not control this, so this wait has no \
                     known end.",
                    c.path.display(),
                    show(c.bytes)
                ),
            },
            None => Judgement::Fits,
        };
    };

    // D8: the floor holds every job but a claim of 0, with a claim or without.
    if rules.floor > 0 && fs.free < rules.floor {
        return Judgement::Outside {
            reason: format!(
                "waits for disk: the filesystem of {} has {} free, and `min_free_disk` is {}. \
                 qex starts no job there below that floor. qex does not control the programs \
                 that fill the disk, so this wait has no known end. qex starts the jobs behind \
                 this one while the space is not free. Free some space. A job that frees space \
                 starts with `--disk 0`.",
                path.display(),
                show(fs.free),
                show(rules.floor)
            ),
        };
    }

    let Some(claim) = claim else {
        return Judgement::Fits;
    };

    // D5. The same test as the submission, in case the configuration changed.
    if let Some(never) = never_text(rules, claim, fs) {
        return Judgement::Never { reason: never };
    }

    // D3 and D4. The claim and the reserve must fit the free space, and so
    // must the claims of the jobs that operate now.
    let own = claim.bytes.saturating_add(rules.reserve);
    let held_here = held_on(held, fs.dev, disks);
    if fs.free >= own.saturating_add(held_here) {
        return Judgement::Fits;
    }

    // The job waits. WHO holds the space? The free space already lacks the
    // bytes that the jobs of this queue wrote, so a test of `free < own` alone
    // blames programs outside the queue for a shortage that a job of the queue
    // causes, and the reader looks for a stranger. Measured on a loop
    // filesystem: a job that claimed 40MB and wrote 36MB left 15MB free, and
    // the second job was told "programs outside this queue hold the space".
    //
    // If the free space and the claims of this queue together would hold the
    // job, the jobs of this queue are the cause and the wait ends when they
    // stop (`Siblings`). If not even that is enough, the space belongs to
    // somebody else (`Outside`).
    if held_here == 0 || fs.free.saturating_add(held_here) < own {
        return Judgement::Outside {
            reason: format!(
                "waits for disk: the filesystem of {} has {} free, and this job claims {} with \
                 {} in reserve (`reserve_disk`). Programs outside this queue, or the files of \
                 earlier jobs, hold the space. qex does not control them, so this wait has no \
                 known end. qex starts the jobs behind this one while the space is not free. \
                 Free some space, or give a smaller --disk.",
                claim.path.display(),
                show(fs.free),
                show(claim.bytes),
                show(rules.reserve)
            ),
        };
    }

    Judgement::Siblings {
        fact: format!(
            "waits for disk: this job claims {} on {}, the filesystem has {} free, the \
             jobs of this queue claim {} there, and {} must stay free (`reserve_disk`).",
            show(claim.bytes),
            claim.path.display(),
            show(fs.free),
            show(held_here),
            show(rules.reserve)
        ),
    }
}

/// Gives the sentence for a claim that no free space can ever satisfy, or
/// `None` when the claim can start some day (D5).
fn never_text(rules: Rules, claim: &DiskClaim, fs: Fs) -> Option<String> {
    if claim.bytes.saturating_add(rules.reserve) <= fs.total {
        return None;
    }
    Some(format!(
        "this job claims {} on {} with {} in reserve (`reserve_disk`), and the filesystem \
         holds {} in total. Give a smaller --disk, lower `reserve_disk` in the configuration, \
         or use another --disk-path.",
        show(claim.bytes),
        claim.path.display(),
        show(rules.reserve),
        show(fs.total)
    ))
}

/// Tests a claim against the size of its filesystem, for the submission.
///
/// A claim that no free space can ever satisfy is an error at the submission
/// and not a wait that never ends (D5). A path that qex cannot read gives no
/// error here: the scheduler says it on every pass.
pub fn check_size(rules: Rules, claim: &DiskClaim, disks: &Disks) -> Result<(), String> {
    if claim.bytes == 0 {
        return Ok(());
    }
    let Some(fs) = disks.stat(&claim.path) else {
        return Ok(());
    };
    match never_text(rules, claim, fs) {
        Some(text) => Err(format!("this job can never start: {text}")),
        None => Ok(()),
    }
}

/// Writes the size of a filesystem for a sentence: `3.1GB`.
pub fn show(bytes: u64) -> String {
    format_size(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn fs(dev: u64, free_gb: u64, total_gb: u64) -> Fs {
        Fs {
            dev,
            free: free_gb * GB,
            total: total_gb * GB,
        }
    }

    fn claim(gb: u64, path: &str) -> DiskClaim {
        DiskClaim {
            bytes: gb * GB,
            path: PathBuf::from(path),
        }
    }

    fn rules(reserve_gb: u64, floor_gb: u64) -> Rules {
        Rules {
            reserve: reserve_gb * GB,
            floor: floor_gb * GB,
        }
    }

    /// Two mounts: `/` with 10GB free of 100GB, and `/mnt/small` with 4GB free
    /// of 8GB.
    fn two_filesystems() -> Disks {
        Disks::fixed(vec![("/", fs(1, 10, 100)), ("/mnt/small", fs(2, 4, 8))])
    }

    /// CLAIM D3: a claim that fits the free space starts.
    #[test]
    fn a_claim_inside_the_free_space_fits() {
        let d = two_filesystems();
        let c = claim(5, "/work");
        assert_eq!(
            judge(rules(1, 0), Some(&c), Path::new("/work"), &[], &d),
            Judgement::Fits
        );
    }

    /// CLAIM D3: the claim and the reserve together must fit. 10GB free, 9GB
    /// claimed and 2GB in reserve is 1GB too much.
    #[test]
    fn the_reserve_is_kept_free_after_the_claim() {
        let d = two_filesystems();
        let c = claim(9, "/work");
        let got = judge(rules(2, 0), Some(&c), Path::new("/work"), &[], &d);
        let Judgement::Outside { reason } = got else {
            panic!("no job holds the space, so the wait has no known end: {got:?}");
        };
        assert!(reason.contains("waits for disk"), "{reason}");
        assert!(
            reason.contains("2GB"),
            "the reason names the reserve: {reason}"
        );
        assert!(reason.contains("reserve_disk"), "{reason}");
    }

    /// CLAIM D3: a claim of exactly the free space less the reserve fits. The
    /// boundary is not a wait.
    #[test]
    fn a_claim_of_exactly_the_free_space_less_the_reserve_fits() {
        let d = two_filesystems();
        let c = claim(8, "/work");
        assert_eq!(
            judge(rules(2, 0), Some(&c), Path::new("/work"), &[], &d),
            Judgement::Fits
        );
    }

    /// CLAIM D3: the claims of the jobs that operate count against the free
    /// space on the same filesystem. 10GB free, 6GB held, 5GB asked.
    #[test]
    fn the_claims_of_running_jobs_count_on_the_same_filesystem() {
        let d = two_filesystems();
        let held = [claim(6, "/other/place")];
        let c = claim(5, "/work");
        let got = judge(rules(0, 0), Some(&c), Path::new("/work"), &held, &d);
        let Judgement::Siblings { fact } = got else {
            panic!("a job of this queue holds the space: {got:?}");
        };
        assert!(fact.contains("waits for disk"), "{fact}");
        assert!(
            fact.contains("6GB"),
            "the fact names what the queue holds: {fact}"
        );
        assert!(fact.contains("5GB"), "the fact names the claim: {fact}");
    }

    /// CLAIM D7: a job that WROTE its claim leaves little free space, and the
    /// next job waits for that job and not for a stranger. Measured on a loop
    /// filesystem: a claim of 40MB, 36MB written, 15MB free; the second job was
    /// told that programs outside the queue hold the space.
    #[test]
    fn a_shortage_that_a_sibling_wrote_is_a_sibling_wait() {
        let d = Disks::fixed(vec![("/", fs(1, 15, 56))]);
        let held = [claim(40, "/work")];
        let c = claim(40, "/work");
        let got = judge(rules(0, 0), Some(&c), Path::new("/work"), &held, &d);
        assert!(
            matches!(got, Judgement::Siblings { .. }),
            "the held claim would cover the gap, so the queue is the cause: {got:?}"
        );
    }

    /// CLAIM D7: when even the claims of this queue could not close the gap,
    /// the space belongs to somebody else, whatever the queue holds.
    #[test]
    fn a_shortage_that_no_release_of_the_queue_can_close_is_outside() {
        let d = Disks::fixed(vec![("/", fs(1, 5, 100))]);
        let held = [claim(4, "/work")];
        let c = claim(50, "/work");
        assert!(matches!(
            judge(rules(0, 0), Some(&c), Path::new("/work"), &held, &d),
            Judgement::Outside { .. }
        ));
    }

    /// CLAIM D3: the sum of the held claims is what counts. Two jobs of 3GB
    /// leave 4GB of 10GB, and a claim of 5GB waits.
    #[test]
    fn several_held_claims_are_added() {
        let d = two_filesystems();
        let held = [claim(3, "/a"), claim(3, "/b")];
        let c = claim(5, "/work");
        assert!(matches!(
            judge(rules(0, 0), Some(&c), Path::new("/work"), &held, &d),
            Judgement::Siblings { .. }
        ));
        let small = claim(4, "/work");
        assert_eq!(
            judge(rules(0, 0), Some(&small), Path::new("/work"), &held, &d),
            Judgement::Fits
        );
    }

    /// CLAIM D2: filesystems are separate. A claim on `/mnt/small` does not
    /// reduce the space of `/`, and the reverse.
    #[test]
    fn two_filesystems_do_not_share_their_claims() {
        let d = two_filesystems();
        let held = [claim(4, "/mnt/small/build")];
        // The held claim fills /mnt/small, and /work is on /.
        let on_root = claim(9, "/work");
        assert_eq!(
            judge(rules(0, 0), Some(&on_root), Path::new("/work"), &held, &d),
            Judgement::Fits,
            "a claim on /mnt/small must not count on /"
        );
        let on_small = claim(1, "/mnt/small/other");
        assert!(
            matches!(
                judge(rules(0, 0), Some(&on_small), Path::new("/"), &held, &d),
                Judgement::Siblings { .. }
            ),
            "a claim on /mnt/small must count on /mnt/small"
        );
    }

    /// CLAIM D2: two paths with different names on one filesystem are ONE
    /// filesystem. The device number decides, and the name does not.
    #[test]
    fn two_paths_on_one_device_share_their_claims() {
        let d = Disks::fixed(vec![
            ("/data", fs(7, 10, 100)),
            ("/home/me/link-to-data", fs(7, 10, 100)),
        ]);
        let held = [claim(8, "/data/a")];
        let c = claim(5, "/home/me/link-to-data/b");
        assert!(matches!(
            judge(rules(0, 0), Some(&c), Path::new("/"), &held, &d),
            Judgement::Siblings { .. }
        ));
    }

    /// CLAIM D5: a claim larger than the filesystem can never start.
    #[test]
    fn a_claim_larger_than_the_filesystem_never_starts() {
        let d = two_filesystems();
        let c = claim(9, "/mnt/small/x");
        let got = judge(rules(0, 0), Some(&c), Path::new("/"), &[], &d);
        let Judgement::Never { reason } = got else {
            panic!("9GB cannot fit a filesystem of 8GB: {got:?}");
        };
        assert!(reason.contains("9GB") && reason.contains("8GB"), "{reason}");
        assert!(reason.contains("/mnt/small/x"), "{reason}");
    }

    /// CLAIM D5: the reserve counts for the size test. 8GB filesystem, 6GB
    /// claim and 3GB reserve can never fit, although the claim alone does.
    #[test]
    fn the_reserve_counts_for_the_size_test() {
        let d = two_filesystems();
        let c = claim(6, "/mnt/small/x");
        assert!(matches!(
            judge(rules(3, 0), Some(&c), Path::new("/"), &[], &d),
            Judgement::Never { .. }
        ));
    }

    /// CLAIM D5: the submission refuses what `judge` would park for ever, and
    /// accepts what can start.
    #[test]
    fn check_size_refuses_a_claim_that_can_never_fit() {
        let d = two_filesystems();
        let err = check_size(rules(1, 0), &claim(8, "/mnt/small/x"), &d)
            .expect_err("8GB and 1GB in reserve cannot fit 8GB");
        assert!(
            err.contains("--disk"),
            "the message names the option: {err}"
        );
        assert!(
            err.contains("reserve_disk"),
            "the message names the remedy: {err}"
        );
        assert!(check_size(rules(1, 0), &claim(7, "/mnt/small/x"), &d).is_ok());
    }

    /// CLAIM D5: the free space now is not the test of the submission. A claim
    /// that is larger than the free space and smaller than the filesystem
    /// waits, and it is not an error.
    #[test]
    fn check_size_does_not_refuse_a_claim_that_waits_for_free_space() {
        let d = two_filesystems();
        // 50GB claim, 10GB free, 100GB total.
        assert!(check_size(rules(1, 0), &claim(50, "/work"), &d).is_ok());
    }

    /// CLAIM D5: a path that qex cannot read is no error at the submission.
    #[test]
    fn check_size_accepts_a_path_it_cannot_read() {
        let d = Disks::with(Box::new(|_| None));
        assert!(check_size(rules(1, 0), &claim(50, "/nowhere"), &d).is_ok());
    }

    /// CLAIM D6: a claim of 0 holds no space and no test holds it, not even
    /// the floor.
    #[test]
    fn a_claim_of_zero_is_exempt_from_every_test() {
        let d = two_filesystems();
        let zero = DiskClaim {
            bytes: 0,
            path: PathBuf::from("/mnt/small/gc"),
        };
        // 4GB free, a floor of 50GB, a reserve of 50GB.
        assert_eq!(
            judge(rules(50, 50), Some(&zero), Path::new("/"), &[], &d),
            Judgement::Fits
        );
    }

    /// CLAIM D6: a claim of 0 holds nothing for the jobs that come after it.
    #[test]
    fn a_held_claim_of_zero_counts_for_nothing() {
        let d = two_filesystems();
        let held = [DiskClaim {
            bytes: 0,
            path: PathBuf::from("/work"),
        }];
        let c = claim(9, "/work");
        assert_eq!(
            judge(rules(0, 0), Some(&c), Path::new("/work"), &held, &d),
            Judgement::Fits
        );
    }

    /// CLAIM D8: the floor holds a job with NO claim, on the filesystem of its
    /// directory.
    #[test]
    fn the_floor_holds_a_job_that_claims_nothing() {
        let d = two_filesystems();
        let got = judge(rules(0, 5), None, Path::new("/mnt/small/proj"), &[], &d);
        let Judgement::Outside { reason } = got else {
            panic!("4GB is under a floor of 5GB: {got:?}");
        };
        assert!(reason.contains("min_free_disk"), "{reason}");
        assert!(reason.contains("4GB") && reason.contains("5GB"), "{reason}");
        assert!(
            reason.contains("--disk 0"),
            "the reason names the way out: {reason}"
        );
    }

    /// CLAIM D8: the floor reads the filesystem of the directory, so the same
    /// floor lets a job on the larger filesystem start.
    #[test]
    fn the_floor_is_tested_on_the_filesystem_of_the_directory() {
        let d = two_filesystems();
        assert_eq!(
            judge(rules(0, 5), None, Path::new("/work"), &[], &d),
            Judgement::Fits,
            "10GB free on / is above a floor of 5GB"
        );
    }

    /// CLAIM D8: the default floor is 0, and it holds nothing.
    #[test]
    fn no_floor_holds_nothing() {
        let d = Disks::fixed(vec![("/", fs(1, 0, 100))]);
        assert_eq!(
            judge(rules(0, 0), None, Path::new("/work"), &[], &d),
            Judgement::Fits,
            "a job with no claim and no floor starts on a full disk, as before"
        );
    }

    /// CLAIM D8: the floor holds a job WITH a claim too. A claim of 1GB on a
    /// filesystem with 4GB free and a floor of 5GB waits.
    #[test]
    fn the_floor_holds_a_job_that_has_a_claim() {
        let d = two_filesystems();
        let c = claim(1, "/mnt/small/x");
        assert!(matches!(
            judge(rules(0, 5), Some(&c), Path::new("/"), &[], &d),
            Judgement::Outside { .. }
        ));
    }

    /// CLAIM D8: a directory that qex cannot read gives no wait for the floor.
    /// The floor is a protection, and it must not stop a job for a fault of
    /// the measurement.
    #[test]
    fn a_directory_that_qex_cannot_read_passes_the_floor() {
        let d = Disks::with(Box::new(|_| None));
        assert_eq!(
            judge(rules(0, 5), None, Path::new("/gone"), &[], &d),
            Judgement::Fits
        );
    }

    /// CLAIM D3: a claim whose path qex cannot read waits and says so. A rule
    /// that the user asked for is applied or refused, and never skipped.
    #[test]
    fn a_claim_on_a_path_that_qex_cannot_read_waits_and_says_why() {
        let d = Disks::with(Box::new(|_| None));
        let c = claim(1, "/gone/away");
        let got = judge(rules(0, 0), Some(&c), Path::new("/"), &[], &d);
        let Judgement::Outside { reason } = got else {
            panic!("an unreadable path must not pass in silence: {got:?}");
        };
        assert!(reason.contains("/gone/away"), "{reason}");
    }

    /// One measurement for each path in one pass.
    #[test]
    fn a_path_is_measured_one_time_for_each_pass() {
        use std::cell::Cell;
        use std::rc::Rc;
        let calls = Rc::new(Cell::new(0));
        let seen = Rc::clone(&calls);
        let d = Disks::with(Box::new(move |_| {
            seen.set(seen.get() + 1);
            Some(fs(1, 10, 100))
        }));
        for _ in 0..5 {
            d.stat(Path::new("/work"));
        }
        d.stat(Path::new("/other"));
        assert_eq!(calls.get(), 2);
    }

    /// The real probe reads a real directory, and a path that does not exist
    /// yet gets the filesystem of the nearest directory above it.
    #[test]
    fn the_system_probe_reads_the_nearest_directory_that_exists() {
        let dir = std::env::temp_dir();
        let here = crate::sys::filesystem(&dir).expect("the temporary directory is readable");
        let below = crate::sys::filesystem(&dir.join("qex-no-such-dir/target/debug"))
            .expect("a path below an existing directory is readable");
        assert_eq!(here.dev, below.dev);
        assert!(here.total > 0 && here.free <= here.total);
    }
}
