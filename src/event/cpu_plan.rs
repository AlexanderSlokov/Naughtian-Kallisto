//! Which logical CPU each worker is pinned to.
//!
//! Thread-per-core (ADR-0016 QĐ-3) is worth having only if each worker gets a
//! core's caches and execution units to itself. Pinning worker `i` to logical
//! CPU `i`, which is what this used to do, does not deliver that: on this
//! project's own benchmark machine logical CPUs 0 and 1 are the two hardware
//! threads of one physical core, so the default two workers shared a core, and
//! on a machine that numbers its siblings differently the same build would
//! behave differently.
//!
//! Three rules, in order:
//!
//! 1. **Stay inside the affinity mask.** Under `taskset` or a Kubernetes CPU
//!    manager, the CPUs this process was given are the only CPUs there are.
//! 2. **One worker per physical core** before any two share one.
//! 3. **Leave CPU 0 for last.** It is where the kernel usually lands its
//!    interrupts, and Kallisto is a guest on somebody else's machine.
//!
//! An operator who knows better sets `spec.cpus` and none of this runs.

use std::{collections::BTreeMap, fs, path::Path};

/// The logical CPUs this process may use, and which physical core each belongs
/// to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topology {
    /// `(logical cpu, (package, core))`, in ascending CPU order.
    cpus: Vec<(usize, (usize, usize))>,
}

impl Topology {
    /// What this machine and this process's affinity mask say.
    ///
    /// Anything unreadable degrades rather than fails: a CPU whose topology is
    /// missing counts as a core of its own, which is the old behaviour, and an
    /// unreadable mask means every CPU is allowed.
    pub fn of_this_process() -> Self {
        Self::read(
            Path::new("/proc/self/status"),
            Path::new("/sys/devices/system/cpu"),
        )
    }

    fn read(status: &Path, sysfs: &Path) -> Self {
        let allowed = fs::read_to_string(status)
            .ok()
            .and_then(|text| {
                text.lines()
                    .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
                    .map(parse_cpu_list)
            })
            .unwrap_or_default();

        let cpus = allowed
            .into_iter()
            .map(|cpu| {
                let core = read_usize(&sysfs.join(format!("cpu{cpu}/topology/core_id")));
                let package =
                    read_usize(&sysfs.join(format!("cpu{cpu}/topology/physical_package_id")));
                // No topology to read: treat the CPU as a core of its own, so
                // the plan degrades to one worker per logical CPU.
                (
                    cpu,
                    core.map_or((usize::MAX, cpu), |core| (package.unwrap_or(0), core)),
                )
            })
            .collect();
        Self { cpus }
    }

    #[cfg(test)]
    fn of(cpus: &[(usize, (usize, usize))]) -> Self {
        Self {
            cpus: cpus.to_vec(),
        }
    }

    /// The allowed CPUs grouped by physical core, each group ordered by CPU
    /// number, the groups ordered by their first CPU — except that the group
    /// holding CPU 0 goes last, unless it is the only one.
    fn cores(&self) -> Vec<Vec<usize>> {
        let mut by_core: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
        for &(cpu, core) in &self.cpus {
            by_core.entry(core).or_default().push(cpu);
        }
        let mut cores: Vec<Vec<usize>> = by_core.into_values().collect();
        cores.sort_by_key(|group| group[0]);
        if cores.len() > 1
            && let Some(first) = cores.iter().position(|group| group.contains(&0))
        {
            let owns_cpu_zero = cores.remove(first);
            cores.push(owns_cpu_zero);
        }
        cores
    }
}

/// One logical CPU per worker, or an empty plan when there is nothing to pin
/// to, which leaves the workers unpinned rather than crowded onto CPU 0.
///
/// More workers than cores wraps around: every core takes a second worker only
/// once every core has a first.
pub fn plan(workers: usize, topology: &Topology) -> Vec<usize> {
    let cores = topology.cores();
    if cores.is_empty() {
        return Vec::new();
    }
    (0..workers)
        .map(|worker| {
            let core = &cores[worker % cores.len()];
            core[(worker / cores.len()) % core.len()]
        })
        .collect()
}

/// `spec.cpus`, when the operator set it: taken as given, and reused in order
/// if there are more workers than entries.
pub fn plan_from(workers: usize, cpus: &[usize]) -> Vec<usize> {
    if cpus.is_empty() {
        return Vec::new();
    }
    (0..workers)
        .map(|worker| cpus[worker % cpus.len()])
        .collect()
}

/// `0-3,6` as the kernel writes it in `Cpus_allowed_list`.
fn parse_cpu_list(list: &str) -> Vec<usize> {
    let mut cpus = Vec::new();
    for part in list.trim().split(',').filter(|part| !part.is_empty()) {
        match part.split_once('-') {
            Some((low, high)) => {
                if let (Ok(low), Ok(high)) = (low.trim().parse(), high.trim().parse::<usize>()) {
                    cpus.extend(low..=high);
                }
            }
            None => {
                if let Ok(cpu) = part.trim().parse() {
                    cpus.push(cpu);
                }
            }
        }
    }
    cpus
}

fn read_usize(path: &Path) -> Option<usize> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Eight logical CPUs, two hardware threads per physical core, numbered the
    /// way the project's own benchmark machine numbers them: 0 and 1 are
    /// siblings. This is the topology that produced the bug.
    fn amd_ryzen_3550h() -> Topology {
        Topology::of(&[
            (0, (0, 0)),
            (1, (0, 0)),
            (2, (0, 1)),
            (3, (0, 1)),
            (4, (0, 2)),
            (5, (0, 2)),
            (6, (0, 3)),
            (7, (0, 3)),
        ])
    }

    /// The same machine as an Intel might number it: siblings are `n` and
    /// `n + 4`. The plan must not care.
    fn siblings_numbered_apart() -> Topology {
        Topology::of(&[
            (0, (0, 0)),
            (1, (0, 1)),
            (2, (0, 2)),
            (3, (0, 3)),
            (4, (0, 0)),
            (5, (0, 1)),
            (6, (0, 2)),
            (7, (0, 3)),
        ])
    }

    #[test]
    fn two_workers_take_two_physical_cores_and_leave_cpu_zero_alone() {
        for topology in [amd_ryzen_3550h(), siblings_numbered_apart()] {
            let pins = plan(2, &topology);
            assert_eq!(pins.len(), 2);
            assert!(!pins.contains(&0), "took CPU 0: {pins:?}");

            let cores: Vec<_> = pins
                .iter()
                .map(|cpu| topology.cpus.iter().find(|(id, _)| id == cpu).unwrap().1)
                .collect();
            assert_ne!(cores[0], cores[1], "both workers on one core: {pins:?}");
        }
    }

    /// The property that matters on any machine, stated once: no core takes a
    /// second worker while another core has none.
    #[test]
    fn every_core_is_used_once_before_any_is_used_twice() {
        let topology = amd_ryzen_3550h();
        for workers in 1..=8 {
            let pins = plan(workers, &topology);
            let mut per_core: BTreeMap<(usize, usize), usize> = BTreeMap::new();
            for cpu in &pins {
                let core = topology.cpus.iter().find(|(id, _)| id == cpu).unwrap().1;
                *per_core.entry(core).or_default() += 1;
            }
            // A core the plan never touched counts as zero, and an untouched
            // core is not in the map, so the total has to be named here.
            let cores = topology.cores().len();
            let most = per_core.values().max().copied().unwrap_or(0);
            let least = if per_core.len() < cores {
                0
            } else {
                *per_core.values().min().unwrap()
            };
            assert!(
                most - least <= 1,
                "{workers} workers spread {pins:?}, {per_core:?}"
            );
            assert_eq!(pins.len(), workers);
        }
    }

    /// Two workers, eight threads each pinned: the second lap doubles up on the
    /// cores already used, and only then is CPU 0 in play.
    #[test]
    fn a_second_worker_per_core_comes_only_after_every_core_has_one() {
        assert_eq!(plan(8, &amd_ryzen_3550h()), vec![2, 4, 6, 0, 3, 5, 7, 1]);
    }

    /// Under `taskset -c 2-3` the two allowed CPUs are siblings. Sharing a core
    /// is then correct: the mask is not ours to widen.
    #[test]
    fn the_affinity_mask_is_never_exceeded() {
        let allowed = Topology::of(&[(2, (0, 1)), (3, (0, 1))]);
        let pins = plan(4, &allowed);
        assert!(pins.iter().all(|cpu| [2, 3].contains(cpu)), "{pins:?}");
        assert_eq!(pins, vec![2, 3, 2, 3]);
    }

    /// A single-core machine has nowhere else to go, so the rule about CPU 0
    /// gives way.
    #[test]
    fn cpu_zero_is_used_when_it_is_the_only_cpu() {
        assert_eq!(plan(2, &Topology::of(&[(0, (0, 0))])), vec![0, 0]);
    }

    /// Sysfs said nothing. Every CPU counts as its own core, which is the old
    /// behaviour, and is still better than crowding one.
    #[test]
    fn an_unreadable_topology_falls_back_to_one_worker_per_logical_cpu() {
        let unknown = Topology::of(&[
            (0, (usize::MAX, 0)),
            (1, (usize::MAX, 1)),
            (2, (usize::MAX, 2)),
        ]);
        assert_eq!(plan(3, &unknown), vec![1, 2, 0]);
    }

    #[test]
    fn nothing_to_pin_to_leaves_the_workers_unpinned() {
        assert!(plan(2, &Topology::of(&[])).is_empty());
        assert!(plan_from(2, &[]).is_empty());
    }

    /// `spec.cpus` is the operator's call and is taken as written.
    #[test]
    fn an_explicit_cpu_list_is_used_in_order() {
        assert_eq!(plan_from(3, &[5, 6]), vec![5, 6, 5]);
    }

    #[test]
    fn the_kernels_cpu_list_syntax_is_read_whole() {
        assert_eq!(parse_cpu_list(" 0-3,6\n"), vec![0, 1, 2, 3, 6]);
        assert_eq!(parse_cpu_list("2"), vec![2]);
        assert_eq!(parse_cpu_list(""), Vec::<usize>::new());
    }

    /// The reader works against files, so it can be pointed at a fake machine.
    #[test]
    fn topology_is_read_from_the_mask_and_sysfs() {
        let dir = std::env::temp_dir().join("kallisto-topology-test");
        let sysfs = dir.join("sys");
        for (cpu, core) in [(2usize, 1usize), (3, 1), (4, 2)] {
            let path = sysfs.join(format!("cpu{cpu}/topology"));
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("core_id"), format!("{core}\n")).unwrap();
            std::fs::write(path.join("physical_package_id"), "0\n").unwrap();
        }
        let status = dir.join("status");
        std::fs::write(&status, "Name:\tx\nCpus_allowed_list:\t2-4\n").unwrap();

        let topology = Topology::read(&status, &sysfs);
        assert_eq!(
            topology,
            Topology::of(&[(2, (0, 1)), (3, (0, 1)), (4, (0, 2))])
        );
        // CPU 0 is not in the mask, so the "leave CPU 0 last" rule has nothing
        // to do and the two cores are used one each.
        assert_eq!(plan(2, &topology), vec![2, 4]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
