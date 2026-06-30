# 网络拓扑

```
+--------------------+       +--------------------+
|  mac (开发主力)    | ----> |  公网 ISP 北京电信  |
|  Darwin 25, M2     |       |  124.126.5.223     |
+--------------------+       +---------+----------+
         |                              |
         |                              | 路由器 NAT
         |                              |   :8443 → 192.168.1.19:8443
         |                              |   :20777 → 192.168.1.19:22
         |                              |   :3389 → 192.168.1.19:3389（救命）
         |                              v
         |                  +-----------+----------+
         +----- LAN ------>|  winhome (Legion)    |
                            |  192.168.1.19         |
                            |  Win11 25H2 + WSL2    |
                            +----------+-----------+
                                       |
                                       v
                            +----------+-----------+
                            | WSL2 Ubuntu 24.04    |
                            | mirrored mode -> 同  |
                            | localhost as Windows |
                            +----------------------+

CF DNS qmledmq.cn (active NS: jermaine + perla.ns.cloudflare.com)
    A     home.qmledmq.cn   -> 124.126.5.223 (ddns-go 维护)
    A     qmledmq.cn        -> 124.126.5.223
    CNAME *.qmledmq.cn       -> home.qmledmq.cn      (三级 wildcard)
    CNAME *.lab.qmledmq.cn   -> home.qmledmq.cn      (四级 wildcard)
```

## 入站链路

| 外部 URL | 端口 | 路由 |
|---|---|---|
| `https://<sub>.qmledmq.cn:8443` | 8443/tcp | NAT → Caddy → reverse_proxy or wildcard placeholder |
| `ssh -p 20777 ... home.qmledmq.cn` | 20777/tcp | NAT → sshd :22 |
| `rdp 124.126.5.223:3389` | 3389/tcp | NAT → Windows RDP（救命用，安全风险） |

`:80` / `:443` 因 ICP 未备案 + ISP 中间盒拦截**不可用**（实测 TLS handshake 被 reset）。

## 内部链路

Caddy reverse_proxy 可达 backend：
- `localhost:<port>`（Windows native service）
- `localhost:<wsl_port>`（mirrored mode 透明反到 WSL）
- `192.168.1.<x>:<port>`（LAN 其他设备）

## DNS 链路

- mac `dig +short <sub>.qmledmq.cn` → 走 mac Clash TUN 解（多数 fake-IP 198.18.x.x，因 mac 上 Clash 拦域名）
- mac `dig +short <sub>.qmledmq.cn @1.1.1.1` → 走 CF 真解析，得 124.126.5.223
- 外部用户 `dig <sub>.qmledmq.cn` → CF NS 返 124.126.5.223

## 引用
- [machines/winhome.md](../machines/winhome.md)
- [machines/mac.md](../machines/mac.md)
- [services/caddy.md](../services/caddy.md)
- [services/sshd.md](../services/sshd.md)
