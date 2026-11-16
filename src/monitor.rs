//! Supervision distante : collecte périodique de l'état d'un serveur Linux
//! (CPU, mémoire, réseau, uptime, utilisateurs, disques) sur un canal SSH
//! dédié, et analyse des blocs renvoyés.
//!
//! Le script tourne dans la même connexion SSH que le terminal : côté serveur,
//! les deux canaux sont des enfants du même processus sshd. Le script retrouve
//! donc le shell interactif (le frère qui possède un terminal) et lit son
//! dossier courant dans `/proc`, ce qui permet à l'explorateur de fichiers de
//! « suivre le terminal ».

use std::time::Instant;

/// Script POSIX exécuté sur le serveur : un bloc de mesures toutes les deux
/// secondes. Il n'utilise que `/proc`, `df`, `who` et `hostname`, présents sur
/// tout Linux courant. Sans guillemets simples pour pouvoir être passé tel quel
/// à `sh -c '…'`.
pub const SCRIPT: &str = r#"while :; do
echo @@BEGIN
echo "host $(hostname 2>/dev/null)"
echo "cpu $(head -n1 /proc/stat 2>/dev/null)"
echo "mem $(grep -E "^(MemTotal|MemAvailable):" /proc/meminfo 2>/dev/null | tr -s " " | tr "\n" " ")"
echo "net $(awk -F "[: ]+" "NR>2 && \$2!=\"lo\" {rx+=\$3; tx+=\$11} END {print rx+0, tx+0}" /proc/net/dev 2>/dev/null)"
echo "up $(cut -d " " -f1 /proc/uptime 2>/dev/null)"
echo "users $(who 2>/dev/null | wc -l) $(id -un 2>/dev/null)"
echo "disk $(df -P 2>/dev/null | awk "NR>1 && \$1 !~ /^(tmpfs|devtmpfs|udev|overlay|squashfs|efivarfs)\$/ {print \$6, \$5}" | tr "\n" ";")"
cwd=""; for d in /proc/[0-9]*; do set -- $(sed "s/^.*) //" "$d/stat" 2>/dev/null); [ "$2" = "$PPID" ] && [ "$5" != "0" ] && cwd=$(readlink "$d/cwd" 2>/dev/null); done
echo "cwd $cwd"
echo "os $(. /etc/os-release 2>/dev/null; echo $PRETTY_NAME)"
echo "ncpu $(grep -c ^processor /proc/cpuinfo 2>/dev/null)"
echo "rootfs $(df -P -k / 2>/dev/null | awk "NR==2 {print \$3, \$2}")"
ps -eo pcpu,rss,comm 2>/dev/null | awk "NR>1 {c=\$1; r=\$2; \$1=\"\"; \$2=\"\"; sub(/^ +/, \"\"); print c, r, \$0}" | sort -nr 2>/dev/null | head -n 5 | while read c r n; do echo "proc $c $r $n"; done
echo @@END
sleep 2
done"#;

/// Commande à exécuter sur le serveur.
pub fn command() -> String {
    format!("exec sh -c '{SCRIPT}'")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub host: String,
    /// Charge CPU en pourcentage, connue à partir du second bloc.
    pub cpu_percent: Option<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    /// Débits en octets par seconde, connus à partir du second bloc.
    pub rx_rate: Option<f64>,
    pub tx_rate: Option<f64>,
    pub uptime_secs: u64,
    pub sessions: u32,
    pub user: String,
    /// (point de montage, pourcentage utilisé)
    pub disks: Vec<(String, u8)>,
    /// Dossier courant du shell du terminal, si le script l'a retrouvé.
    pub cwd: Option<String>,
    /// Nom de la distribution (`PRETTY_NAME` de `/etc/os-release`).
    pub os: String,
    pub cpus: u32,
    /// Système de fichiers racine, en octets.
    pub root_used: u64,
    pub root_total: u64,
    /// Processus les plus gourmands en CPU.
    pub processes: Vec<Process>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Process {
    pub name: String,
    pub cpu_percent: f32,
    pub rss: u64,
}

#[derive(Clone, Copy)]
struct Sample {
    at: Instant,
    cpu_idle: u64,
    cpu_total: u64,
    rx: u64,
    tx: u64,
}

/// Analyseur avec mémoire du bloc précédent (nécessaire pour les deltas).
#[derive(Default)]
pub struct Parser {
    buffer: String,
    previous: Option<Sample>,
}

impl Parser {
    /// Ajoute des octets reçus et renvoie les mesures des blocs complets.
    pub fn feed(&mut self, data: &[u8]) -> Vec<Stats> {
        self.buffer.push_str(&String::from_utf8_lossy(data));
        let mut out = Vec::new();
        while let Some(end) = self.buffer.find("@@END") {
            let block = self.buffer[..end].to_string();
            self.buffer.drain(..end + "@@END".len());
            if let Some(start) = block.find("@@BEGIN") {
                out.push(self.parse_block(&block[start + "@@BEGIN".len()..], Instant::now()));
            }
        }
        if self.buffer.len() > 64 * 1024 {
            self.buffer.clear();
        }
        out
    }

    fn parse_block(&mut self, block: &str, now: Instant) -> Stats {
        let mut stats = Stats::default();
        let mut sample = Sample { at: now, cpu_idle: 0, cpu_total: 0, rx: 0, tx: 0 };
        let mut have_cpu = false;
        let mut have_net = false;

        for line in block.lines() {
            let line = line.trim();
            let Some((key, value)) = line.split_once(' ') else { continue };
            match key {
                "host" => stats.host = value.trim().to_string(),
                "cpu" => {
                    // "cpu user nice system idle iowait irq softirq steal …"
                    let fields: Vec<u64> = value
                        .split_whitespace()
                        .skip(1)
                        .filter_map(|f| f.parse().ok())
                        .collect();
                    if fields.len() >= 4 {
                        sample.cpu_idle = fields[3] + fields.get(4).copied().unwrap_or(0);
                        sample.cpu_total = fields.iter().sum();
                        have_cpu = true;
                    }
                }
                "mem" => {
                    let mut total = 0;
                    let mut available = 0;
                    let fields: Vec<&str> = value.split_whitespace().collect();
                    for pair in fields.chunks(3) {
                        if let [name, kb, ..] = pair {
                            let bytes = kb.parse::<u64>().unwrap_or(0) * 1024;
                            match *name {
                                "MemTotal:" => total = bytes,
                                "MemAvailable:" => available = bytes,
                                _ => {}
                            }
                        }
                    }
                    stats.mem_total = total;
                    stats.mem_used = total.saturating_sub(available);
                }
                "net" => {
                    let mut it = value.split_whitespace().filter_map(|f| f.parse::<u64>().ok());
                    if let (Some(rx), Some(tx)) = (it.next(), it.next()) {
                        sample.rx = rx;
                        sample.tx = tx;
                        have_net = true;
                    }
                }
                "up" => stats.uptime_secs = value.trim().parse::<f64>().unwrap_or(0.0) as u64,
                "users" => {
                    let mut it = value.split_whitespace();
                    stats.sessions = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                    stats.user = it.next().unwrap_or_default().to_string();
                }
                "cwd" => {
                    let path = value.trim();
                    if path.starts_with('/') {
                        stats.cwd = Some(path.to_string());
                    }
                }
                "os" => stats.os = value.trim().to_string(),
                "ncpu" => stats.cpus = value.trim().parse().unwrap_or(0),
                "rootfs" => {
                    let mut it = value.split_whitespace().filter_map(|f| f.parse::<u64>().ok());
                    if let (Some(used), Some(total)) = (it.next(), it.next()) {
                        stats.root_used = used * 1024;
                        stats.root_total = total * 1024;
                    }
                }
                "proc" => {
                    let mut it = value.splitn(3, char::is_whitespace);
                    let cpu = it.next().and_then(|c| c.parse::<f32>().ok());
                    let rss = it.next().and_then(|r| r.parse::<u64>().ok());
                    let name = it.next().unwrap_or_default().trim();
                    if let (Some(cpu_percent), Some(rss)) = (cpu, rss) {
                        if !name.is_empty() {
                            stats.processes.push(Process { name: name.to_string(), cpu_percent, rss: rss * 1024 });
                        }
                    }
                }
                "disk" => {
                    stats.disks = value
                        .split(';')
                        .filter_map(|entry| {
                            let (mount, pct) = entry.trim().rsplit_once(' ')?;
                            let pct = pct.trim_end_matches('%').parse::<u8>().ok()?;
                            Some((mount.to_string(), pct))
                        })
                        .collect();
                }
                _ => {}
            }
        }

        if let Some(prev) = self.previous {
            let elapsed = now.duration_since(prev.at).as_secs_f64();
            if have_cpu && sample.cpu_total > prev.cpu_total {
                let total = (sample.cpu_total - prev.cpu_total) as f32;
                let idle = sample.cpu_idle.saturating_sub(prev.cpu_idle) as f32;
                stats.cpu_percent = Some(((1.0 - idle / total) * 100.0).clamp(0.0, 100.0));
            }
            if have_net && elapsed > 0.0 {
                stats.rx_rate = Some(sample.rx.saturating_sub(prev.rx) as f64 / elapsed);
                stats.tx_rate = Some(sample.tx.saturating_sub(prev.tx) as f64 / elapsed);
            }
        }
        if have_cpu || have_net {
            self.previous = Some(sample);
        }
        stats
    }
}

/// Durée d'activité lisible : « 12 j 3 h », « 3 h 07 », « 12 min ».
pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days} j {hours} h")
    } else if hours > 0 {
        format!("{hours} h {minutes:02}")
    } else {
        format!("{minutes} min")
    }
}

/// Débit en bits par seconde, comme les outils réseau : « 0,06 Mb/s ».
pub fn format_rate(bytes_per_sec: f64) -> String {
    let bits = bytes_per_sec * 8.0;
    if bits >= 1e9 {
        format!("{:.2} Gb/s", bits / 1e9)
    } else if bits >= 1e4 {
        format!("{:.2} Mb/s", bits / 1e6)
    } else {
        format!("{:.0} kb/s", bits / 1e3)
    }
}

/// Débit en octets par seconde : « 3.0 Mo/s ».
pub fn format_bytes_rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1e9 {
        format!("{:.1} Go/s", bytes_per_sec / 1e9)
    } else if bytes_per_sec >= 1e6 {
        format!("{:.1} Mo/s", bytes_per_sec / 1e6)
    } else if bytes_per_sec >= 1e3 {
        format!("{:.0} Ko/s", bytes_per_sec / 1e3)
    } else {
        format!("{bytes_per_sec:.0} o/s")
    }
}

/// Taille mémoire courte : « 184 Mo », « 1.2 Go ».
pub fn format_mem(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1024.0 {
        format!("{:.1} Go", mb / 1024.0)
    } else {
        format!("{mb:.0} Mo")
    }
}

pub fn format_gb(bytes: u64) -> String {
    format!("{:.2} Go", bytes as f64 / 1_073_741_824.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const BLOCK_1: &str = "@@BEGIN\nhost pi\ncpu cpu 100 0 100 800 0 0 0 0 0 0\nmem MemTotal: 4000000 kB MemAvailable: 3000000 kB \nnet 1000 2000\nup 86400.50\nusers 5 deployer\ndisk / 68%;/tmp 0%;/boot/firmware 14%;\ncwd /home/deployer/app\nos Debian GNU/Linux 12 (bookworm)\nncpu 4\nrootfs 51200000 125000000\nproc 12.4 188416 nginx: worker process\nproc 0.3 8192 sshd\n@@END\n";
    const BLOCK_2: &str = "@@BEGIN\nhost pi\ncpu cpu 150 0 150 900 0 0 0 0 0 0\nmem MemTotal: 4000000 kB MemAvailable: 3000000 kB \nnet 1000 2000\nup 86402.50\nusers 5 deployer\ndisk / 68%;\n@@END\n";

    #[test]
    fn first_block_has_static_values_but_no_rates() {
        let mut parser = Parser::default();
        let stats = parser.feed(BLOCK_1.as_bytes());
        assert_eq!(stats.len(), 1);
        let s = &stats[0];
        assert_eq!(s.host, "pi");
        assert_eq!(s.cpu_percent, None);
        assert_eq!(s.rx_rate, None);
        assert_eq!(s.mem_total, 4_096_000_000);
        assert_eq!(s.mem_used, 1_024_000_000);
        assert_eq!(s.uptime_secs, 86_400);
        assert_eq!((s.sessions, s.user.as_str()), (5, "deployer"));
        assert_eq!(s.disks, vec![("/".into(), 68), ("/tmp".into(), 0), ("/boot/firmware".into(), 14)]);
        assert_eq!(s.cwd.as_deref(), Some("/home/deployer/app"));
        assert_eq!(s.os, "Debian GNU/Linux 12 (bookworm)");
        assert_eq!(s.cpus, 4);
        assert_eq!((s.root_used, s.root_total), (51_200_000 * 1024, 125_000_000 * 1024));
        assert_eq!(s.processes.len(), 2);
        assert_eq!(s.processes[0], Process { name: "nginx: worker process".into(), cpu_percent: 12.4, rss: 188_416 * 1024 });
    }

    #[test]
    fn second_block_computes_cpu_from_deltas() {
        let mut parser = Parser::default();
        let t0 = Instant::now();
        parser.parse_block(&BLOCK_1["@@BEGIN".len()..BLOCK_1.len() - "@@END\n".len()], t0);
        let s = parser.parse_block(
            &BLOCK_2["@@BEGIN".len()..BLOCK_2.len() - "@@END\n".len()],
            t0 + Duration::from_secs(2),
        );
        // Delta : 200 au total dont 100 inactifs → 50 %.
        assert!((s.cpu_percent.unwrap() - 50.0).abs() < 0.1);
        assert_eq!(s.rx_rate, Some(0.0));
        assert_eq!(s.cwd, None, "bloc sans ligne cwd");
    }

    #[test]
    fn partial_chunks_are_reassembled() {
        let mut parser = Parser::default();
        let (a, b) = BLOCK_1.split_at(40);
        assert!(parser.feed(a.as_bytes()).is_empty());
        assert_eq!(parser.feed(b.as_bytes()).len(), 1);
    }

    #[test]
    fn formatting() {
        assert_eq!(format_uptime(12 * 86_400 + 3 * 3_600), "12 j 3 h");
        assert_eq!(format_uptime(3 * 3_600 + 7 * 60), "3 h 07");
        assert_eq!(format_uptime(720), "12 min");
        assert_eq!(format_rate(7_500.0), "0.06 Mb/s");
        assert_eq!(format_rate(100.0), "1 kb/s");
        assert_eq!(format_gb(1_073_741_824), "1.00 Go");
        assert_eq!(format_bytes_rate(3_000_000.0), "3.0 Mo/s");
        assert_eq!(format_bytes_rate(2_600.0), "3 Ko/s");
        assert_eq!(format_mem(188_416 * 1024), "184 Mo");
        assert_eq!(format_mem(1_288_490_189), "1.2 Go");
    }

    #[test]
    fn script_has_no_single_quotes() {
        assert!(!SCRIPT.contains('\''), "le script est passé dans sh -c '…'");
    }
}
