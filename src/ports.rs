use crate::state::{ProcessMetadata, ProcessStatus, StateManager};
use nix::sys::signal;
use nix::unistd::Pid;
use std::collections::{HashMap, HashSet};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortEntry {
    pub protocol: Protocol,
    pub address: String,
    pub port: u16,
    pub pid: i32,
    pub command: String,
    pub user: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct RunaMatch {
    pub name: String,
    pub status: ProcessStatus,
    pub cmd: String,
    pub supervisor_pid: i32,
    pub child_pid: Option<i32>,
    pub restart_count: u64,
    pub last_exit_code: Option<i32>,
    pub last_exit_at: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct PortRow {
    pub entry: PortEntry,
    pub runa: Option<RunaMatch>,
    pub conflict_count: usize,
}

#[derive(Debug, Clone)]
pub struct PortSnapshot {
    pub rows: Vec<PortRow>,
    pub refreshed_at: SystemTime,
    pub errors: Vec<String>,
}

impl PortSnapshot {
    pub fn tcp_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.entry.protocol == Protocol::Tcp)
            .count()
    }

    pub fn udp_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.entry.protocol == Protocol::Udp)
            .count()
    }

    pub fn runa_count(&self) -> usize {
        self.rows.iter().filter(|row| row.runa.is_some()).count()
    }

    pub fn conflict_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.conflict_count > 1)
            .count()
    }
}

pub fn collect_snapshot(state_manager: &StateManager, include_udp: bool) -> PortSnapshot {
    let mut errors = Vec::new();
    let mut entries = Vec::new();

    match collect_lsof_tcp() {
        Ok(mut tcp) => entries.append(&mut tcp),
        Err(err) => errors.push(err),
    }

    if include_udp {
        match collect_lsof_udp() {
            Ok(mut udp) => entries.append(&mut udp),
            Err(err) => errors.push(err),
        }
    }

    let entries = collapse_duplicate_bindings(entries);
    let processes = state_manager.list_processes().unwrap_or_default();
    let pid_map = build_runa_pid_map(&processes);
    let conflict_counts = build_conflict_counts(&entries);

    let rows = entries
        .into_iter()
        .map(|entry| {
            let runa = pid_map.get(&entry.pid).cloned();
            let conflict_count = *conflict_counts
                .get(&(entry.protocol, entry.port))
                .unwrap_or(&1);
            PortRow {
                entry,
                runa,
                conflict_count,
            }
        })
        .collect();

    PortSnapshot {
        rows,
        refreshed_at: SystemTime::now(),
        errors,
    }
}

pub fn collect_lsof_tcp() -> Result<Vec<PortEntry>, String> {
    match run_command_with_timeout("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN"], COMMAND_TIMEOUT) {
        Ok(output) if output.status.success() => Ok(parse_lsof_output(
            &String::from_utf8_lossy(&output.stdout),
            Protocol::Tcp,
        )),
        Ok(output) => {
            let lsof_err = format!(
                "lsof TCP failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            match collect_ss_tcp() {
                Ok(entries) => Ok(entries),
                Err(ss_err) => Err(format!("{lsof_err}; {ss_err}")),
            }
        }
        Err(lsof_err) => match collect_ss_tcp() {
            Ok(entries) => Ok(entries),
            Err(ss_err) => Err(format!("{lsof_err}; {ss_err}")),
        },
    }
}

pub fn collect_lsof_udp() -> Result<Vec<PortEntry>, String> {
    match run_command_with_timeout("lsof", &["-nP", "-iUDP"], COMMAND_TIMEOUT) {
        Ok(output) if output.status.success() => Ok(parse_lsof_output(
            &String::from_utf8_lossy(&output.stdout),
            Protocol::Udp,
        )),
        Ok(output) => {
            let lsof_err = format!(
                "lsof UDP failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            match collect_ss_udp() {
                Ok(entries) => Ok(entries),
                Err(ss_err) => Err(format!("{lsof_err}; {ss_err}")),
            }
        }
        Err(lsof_err) => match collect_ss_udp() {
            Ok(entries) => Ok(entries),
            Err(ss_err) => Err(format!("{lsof_err}; {ss_err}")),
        },
    }
}

pub fn parse_lsof_output(output: &str, protocol: Protocol) -> Vec<PortEntry> {
    output
        .lines()
        .skip(1)
        .filter_map(|line| parse_lsof_line(line, protocol))
        .collect()
}

fn parse_lsof_line(line: &str, protocol: Protocol) -> Option<PortEntry> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 9 {
        return None;
    }

    let pid = parts.get(1)?.parse::<i32>().ok()?;
    let protocol_token_index = parts
        .iter()
        .position(|part| *part == "TCP" || *part == "UDP")?;
    let address_token = *parts.get(protocol_token_index + 1)?;
    let local_address = address_token.split("->").next().unwrap_or(address_token);
    let (address, port) = parse_local_address(local_address)?;

    Some(PortEntry {
        protocol,
        address,
        port,
        pid,
        command: parts[0].to_string(),
        user: parts[2].to_string(),
        source: "system".to_string(),
    })
}

fn parse_local_address(value: &str) -> Option<(String, u16)> {
    let value = value.trim();
    let (address, port_str) = value.rsplit_once(':')?;
    let port = port_str.parse::<u16>().ok()?;
    let address = match address {
        "*" | "::" | "[::]" => "0.0.0.0",
        other => other.trim_matches(['[', ']']),
    };
    Some((address.to_string(), port))
}

fn collect_ss_tcp() -> Result<Vec<PortEntry>, String> {
    let output = run_command_with_timeout("ss", &["-tunlp"], COMMAND_TIMEOUT)?;
    if !output.status.success() {
        return Err(format!(
            "ss TCP failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(parse_ss_output(
        &String::from_utf8_lossy(&output.stdout),
        true,
        false,
    ))
}

fn collect_ss_udp() -> Result<Vec<PortEntry>, String> {
    let output = run_command_with_timeout("ss", &["-tunlp"], COMMAND_TIMEOUT)?;
    if !output.status.success() {
        return Err(format!(
            "ss UDP failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(parse_ss_output(
        &String::from_utf8_lossy(&output.stdout),
        false,
        true,
    ))
}

pub fn parse_ss_output(output: &str, include_tcp: bool, include_udp: bool) -> Vec<PortEntry> {
    output
        .lines()
        .skip(1)
        .filter_map(|line| parse_ss_line(line, include_tcp, include_udp))
        .collect()
}

fn parse_ss_line(line: &str, include_tcp: bool, include_udp: bool) -> Option<PortEntry> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 5 {
        return None;
    }

    let protocol = match parts[0].to_ascii_lowercase().as_str() {
        proto if proto.starts_with("tcp") && include_tcp => Protocol::Tcp,
        proto if proto.starts_with("udp") && include_udp => Protocol::Udp,
        _ => return None,
    };

    let local_address = parts
        .iter()
        .find(|part| part.contains(':') && !part.contains("users:"))?;
    let (address, port) = parse_local_address(local_address)?;
    let users = parts.iter().find(|part| part.contains("users:")).copied();
    let (command, pid) = parse_ss_users(users);

    Some(PortEntry {
        protocol,
        address,
        port,
        pid,
        command,
        user: "-".to_string(),
        source: "system".to_string(),
    })
}

fn parse_ss_users(users: Option<&str>) -> (String, i32) {
    let Some(users) = users else {
        return ("unknown".to_string(), 0);
    };

    let command = users
        .split('"')
        .nth(1)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_string();

    let pid = users
        .split("pid=")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);

    (command, pid)
}

fn run_command_with_timeout(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to run {program}: {err}"))?;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|err| format!("failed to read {program} output: {err}"));
            }
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} timed out after {}ms",
                    timeout.as_millis()
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(err) => return Err(format!("failed to wait for {program}: {err}")),
        }
    }
}

pub fn collapse_duplicate_bindings(entries: Vec<PortEntry>) -> Vec<PortEntry> {
    let mut seen = HashMap::<(Protocol, u16, i32, String), PortEntry>::new();
    for entry in entries {
        let key = (
            entry.protocol,
            entry.port,
            entry.pid,
            entry.command.to_lowercase(),
        );

        match seen.get(&key) {
            Some(existing) if binding_score(existing) >= binding_score(&entry) => {}
            _ => {
                seen.insert(key, entry);
            }
        }
    }

    let mut entries: Vec<_> = seen.into_values().collect();
    entries.sort_by_key(|entry| (entry.port, entry.protocol.as_str(), entry.pid));
    entries
}

fn binding_score(entry: &PortEntry) -> u8 {
    match entry.address.as_str() {
        "0.0.0.0" => 3,
        "127.0.0.1" => 2,
        "::1" => 1,
        _ => 0,
    }
}

pub fn build_conflict_counts(entries: &[PortEntry]) -> HashMap<(Protocol, u16), usize> {
    let mut seen = HashSet::<(Protocol, u16, i32, String)>::new();
    let mut counts = HashMap::<(Protocol, u16), usize>::new();

    for entry in entries {
        let unique_binding = (
            entry.protocol,
            entry.port,
            entry.pid,
            entry.command.to_lowercase(),
        );
        if seen.insert(unique_binding) {
            *counts.entry((entry.protocol, entry.port)).or_insert(0) += 1;
        }
    }

    counts
}

pub fn build_runa_pid_map(processes: &[ProcessMetadata]) -> HashMap<i32, RunaMatch> {
    let mut pid_map = HashMap::new();
    for process in processes {
        let runa_match = RunaMatch {
            name: process.name.clone(),
            status: process.status.clone(),
            cmd: process.cmd.clone(),
            supervisor_pid: process.pid,
            child_pid: process.child_pid,
            restart_count: process.restart_count,
            last_exit_code: process.last_exit_code,
            last_exit_at: process.last_exit_at,
        };

        pid_map.insert(process.pid, runa_match.clone());
        if let Some(child_pid) = process.child_pid {
            pid_map.insert(child_pid, runa_match.clone());
            for member_pid in process_group_members(child_pid) {
                pid_map.insert(member_pid, runa_match.clone());
            }
        }
    }
    pid_map
}

fn process_group_members(pgid: i32) -> Vec<i32> {
    let Ok(output) = run_command_with_timeout("pgrep", &["-g", &pgid.to_string()], COMMAND_TIMEOUT)
    else {
        return Vec::new();
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.parse::<i32>().ok())
        .collect()
}

pub fn is_process_alive(pid: i32) -> bool {
    pid > 1 && signal::kill(Pid::from_raw(pid), None).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(protocol: Protocol, port: u16, pid: i32, command: &str, address: &str) -> PortEntry {
        PortEntry {
            protocol,
            address: address.to_string(),
            port,
            pid,
            command: command.to_string(),
            user: "manish".to_string(),
            source: "system".to_string(),
        }
    }

    #[test]
    fn parses_macos_tcp_lsof_output() {
        let output = "COMMAND    PID   USER   FD   TYPE DEVICE SIZE/OFF NODE NAME\nrapportd  1059 manish   10u  IPv4 0xabc      0t0  TCP *:62688 (LISTEN)\nzed       2241 manish   20u  IPv4 0xdef      0t0  TCP 127.0.0.1:44438 (LISTEN)\n";

        let ports = parse_lsof_output(output, Protocol::Tcp);

        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].protocol, Protocol::Tcp);
        assert_eq!(ports[0].address, "0.0.0.0");
        assert_eq!(ports[0].port, 62688);
        assert_eq!(ports[0].pid, 1059);
        assert_eq!(ports[1].address, "127.0.0.1");
    }

    #[test]
    fn parses_udp_and_skips_non_numeric_ports() {
        let output = "COMMAND     PID   USER   FD   TYPE DEVICE SIZE/OFF NODE NAME\nrapportd   1059 manish   21u  IPv6 0xabc      0t0  UDP *:3722\nidentitys  1080 manish   10u  IPv4 0xdef      0t0  UDP *:*\nGoogle    30855 manish   29u  IPv4 0xghi      0t0  UDP 192.168.0.100:56594->142.250.76.67:443\n";

        let ports = parse_lsof_output(output, Protocol::Udp);

        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].port, 3722);
        assert_eq!(ports[1].address, "192.168.0.100");
        assert_eq!(ports[1].port, 56594);
    }

    #[test]
    fn parses_linux_ss_output() {
        let output = "Netid State  Recv-Q Send-Q Local Address:Port Peer Address:Port Process\nudp   UNCONN 0      0          0.0.0.0:5353      0.0.0.0:*     users:((\"mdns\",pid=101,fd=7))\ntcp   LISTEN 0      128        127.0.0.1:3000    0.0.0.0:*     users:((\"node\",pid=202,fd=18))\n";

        let tcp = parse_ss_output(output, true, false);
        let udp = parse_ss_output(output, false, true);

        assert_eq!(tcp.len(), 1);
        assert_eq!(tcp[0].command, "node");
        assert_eq!(tcp[0].pid, 202);
        assert_eq!(tcp[0].port, 3000);
        assert_eq!(udp.len(), 1);
        assert_eq!(udp[0].port, 5353);
    }

    #[test]
    fn conflict_counts_ignore_duplicate_ipv4_ipv6_bindings() {
        let entries = collapse_duplicate_bindings(vec![
            entry(Protocol::Tcp, 5000, 10, "node", "0.0.0.0"),
            entry(Protocol::Tcp, 5000, 10, "node", "::"),
            entry(Protocol::Tcp, 5000, 11, "python", "127.0.0.1"),
            entry(Protocol::Udp, 5000, 12, "mdns", "0.0.0.0"),
        ]);

        let counts = build_conflict_counts(&entries);

        assert_eq!(entries.len(), 3);
        assert_eq!(counts.get(&(Protocol::Tcp, 5000)), Some(&2));
        assert_eq!(counts.get(&(Protocol::Udp, 5000)), Some(&1));
    }

    #[test]
    fn maps_runa_supervisor_and_child_pids() {
        let processes = vec![ProcessMetadata {
            schema_version: 1,
            name: "api".to_string(),
            pid: 100,
            child_pid: Some(200),
            cmd: "node server.js".to_string(),
            status: ProcessStatus::Running,
            created_at: 0,
            restart_count: 0,
            last_exit_code: None,
            last_exit_at: None,
        }];

        let pid_map = build_runa_pid_map(&processes);

        assert_eq!(pid_map.get(&100).unwrap().name, "api");
        assert_eq!(pid_map.get(&200).unwrap().cmd, "node server.js");
    }
}
