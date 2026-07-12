# aidememo-napi-linux-arm64-gnu

Prebuilt native binary used by [`aidememo-napi`](https://www.npmjs.com/package/aidememo-napi)
on arm64 Linux distributions that use glibc.

| Property | Value |
|---|---|
| Operating system | Linux |
| CPU | `arm64` |
| C library | glibc |
| Node.js | 16 or newer |
| Binary | `aidememo-napi.linux-arm64-gnu.node` |

Do not install this package directly. Install `aidememo-napi`; npm will select
this optional dependency automatically for a compatible machine. Alpine Linux
and other musl-based distributions are not supported by this package.

```bash
npm install aidememo-napi
```

[Documentation](https://aidememo.taeyun.me) ·
[Source](https://github.com/taeyun16/aidememo) ·
[Issues](https://github.com/taeyun16/aidememo/issues)
