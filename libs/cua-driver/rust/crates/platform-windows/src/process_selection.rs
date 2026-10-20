#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
}

pub(crate) fn descendants_from_processes(root_pid: u32, all: &[ProcessInfo]) -> Vec<u32> {
    let mut result = vec![root_pid];
    let mut frontier = vec![root_pid];
    while let Some(parent) = frontier.pop() {
        for p in all {
            if p.parent_pid == parent && !result.contains(&p.pid) {
                result.push(p.pid);
                frontier.push(p.pid);
            }
        }
    }
    result
}

pub(crate) fn related_processes_from_processes(
    root_pid: u32,
    _exe_basename: &str,
    all: &[ProcessInfo],
) -> Vec<u32> {
    descendants_from_processes(root_pid, all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, parent_pid: u32, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid,
            name: name.into(),
        }
    }

    #[test]
    fn descendants_include_root_and_only_its_transitive_process_tree() {
        let processes = [
            process(42, 1, "process-42.exe"),
            process(43, 42, "process-43.exe"),
            process(44, 43, "process-44.exe"),
            process(45, 42, "process-45.exe"),
            process(99, 1, "process-99.exe"),
            process(100, 99, "process-100.exe"),
        ];
        assert_eq!(
            descendants_from_processes(42, &processes),
            vec![42, 43, 45, 44]
        );
    }

    #[test]
    fn launch_candidates_exclude_unrelated_same_name_processes() {
        let processes = [
            process(42, 1, "maya.exe"),
            process(99, 1, "maya.exe"),
            process(100, 1, "maya.exe"),
        ];
        assert_eq!(
            related_processes_from_processes(42, "maya.exe", &processes),
            vec![42]
        );
    }

    #[test]
    fn launch_candidates_include_renamed_descendants() {
        let processes = [
            process(42, 1, "maya.exe"),
            process(43, 42, "maya-helper.exe"),
            process(44, 43, "python.exe"),
        ];
        assert_eq!(
            related_processes_from_processes(42, "maya.exe", &processes),
            vec![42, 43, 44]
        );
    }

    #[test]
    fn launch_candidates_keep_exited_root_first() {
        let processes = [process(43, 42, "python.exe")];
        assert_eq!(
            related_processes_from_processes(42, "maya.exe", &processes),
            vec![42, 43]
        );
        assert_eq!(
            related_processes_from_processes(42, "maya.exe", &[]),
            vec![42]
        );
    }
}
