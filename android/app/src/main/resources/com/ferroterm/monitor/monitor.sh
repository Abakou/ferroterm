while :; do
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
done
