# Despliegue en Raspberry Pi (bot de carry)

Documentación del despliegue de **mcPato** en una Raspberry Pi de la red local,
corriendo como servicio `systemd` de forma permanente. Tras la búsqueda de edge
(ver [`docs/EXPERIMENTOS.md`](./docs/EXPERIMENTOS.md)), la Pi corre el **bot de
carry delta-neutral** (ver [`docs/CARRY_BOT.md`](./docs/CARRY_BOT.md)), que
sustituyó al organismo evolutivo (cuyo enfoque predictivo no generaba renta).

> Los datos personales (usuario, IP, contraseñas) se omiten a propósito.
> Sustituye los placeholders `<usuario>` y `<ip-pi>` por los tuyos.
>
> Última actualización: 2026-06-25

---

## 1. Resumen

| Dato | Valor |
|------|-------|
| **Host** | `raspberrypi.local` (resuelve por mDNS; o la IP fija `<ip-pi>` en la LAN) |
| **Usuario** | `<usuario>` (el login SSH va con clave; `sudo` pide la contraseña de ese usuario) |
| **SO** | Debian GNU/Linux 13 (trixie), `aarch64` |
| **Hardware** | 4 núcleos, ~8 GB RAM (Pi 4/5) |
| **Toolchain** | Rust 1.96.0 (instalado vía `rustup`, perfil `minimal`) |
| **Directorio** | `/home/<usuario>/mcpato` |
| **Binario** | `/home/<usuario>/mcpato/target/release/mcpato` (release ARM64, ~10 MB) |
| **Servicio** | `mcpato.service` (systemd, habilitado en arranque) |
| **Modo** | **Carry** (`MCPATO_CARRY_BOT=true` en `.env`) · delta-neutral · paper · 1x |
| **Dashboard** | http://raspberrypi.local:8090 (carry: equity, funding, posición) |
| **Instrumento** | BTCUSDT (spot + perp) · notificaciones Telegram activas |

---

## 2. Estructura en disco y permisos

Todo el proyecto pertenece al usuario de despliegue (`<usuario>`):

```
/home/<usuario>/mcpato/                 drwxr-xr-x
├── src/                                fuente Rust
├── target/release/mcpato               -rwxrwxr-x   (binario compilado)
├── data/                               drwxrwxr-x
│   ├── mcpato.sqlite                   -rw-r--r--   (estado del carry, persistente)
│   └── mcpato.sqlite.bak-AAAA-MM-DD    -rw-r--r--   (backup del organismo anterior)
├── .env                                -rw-r--r--   (config + token Telegram — NO en git)
├── Cargo.toml / Cargo.lock
└── ...

/etc/systemd/system/mcpato.service      -rw-r--r--  root:root   (unit del servicio)
```

Notas:
- **`data/mcpato.sqlite`** guarda el estado vivo del carry (tabla `carry_state`:
  equity, funding acumulado, última liquidación cobrada, posición abierta/cash).
  Sobrevive a reinicios → el bot continúa donde lo dejó. Es lo que conviene
  respaldar.
- El **`.bak`** es la BD del organismo evolutivo anterior, guardada por si se
  quisiera volver a ese modo.
- **`.env`** contiene el token de Telegram; está en `.gitignore` y se copió
  manualmente (no viene del repo). **No se sube a GitHub.** Es la **única** fuente
  de configuración (incluido `MCPATO_CARRY_BOT=true`), por eso el `rsync` de
  despliegue lo excluye (ver §6).
- El **unit** es propiedad de `root` porque vive en `/etc/systemd/system`.

---

## 3. El servicio systemd

Archivo: `/etc/systemd/system/mcpato.service` (sustituye `<usuario>`):

```ini
[Unit]
Description=mcPato - bot de carry delta-neutral
After=network-online.target
Wants=network-online.target
StartLimitIntervalSec=0

[Service]
Type=simple
User=<usuario>
WorkingDirectory=/home/<usuario>/mcpato
ExecStart=/home/<usuario>/mcpato/target/release/mcpato
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

> El **modo** (carry vs organismo) NO está en el unit, sino en `.env`
> (`MCPATO_CARRY_BOT=true`). Para volver al organismo: quitar esa línea y
> reiniciar (ver §8 sobre la BD).

Claves del diseño:
- **`WorkingDirectory`** — imprescindible: la app lee `.env` y escribe
  `data/mcpato.sqlite` con rutas relativas al directorio de trabajo.
- **`Restart=always` + `RestartSec=5`** — se reinicia solo tras cualquier salida
  (crash, fallo persistente). Como recarga el estado de SQLite, retoma el carry
  donde estaba (sin recobrar funding ya cobrado).
- **`StartLimitIntervalSec=0`** — desactiva el límite de reintentos de systemd,
  para que `Restart=always` nunca quede bloqueado.
- **`After/Wants network-online.target`** — espera a la red antes de arrancar
  (lee el funding por la API REST de Binance).
- **`WantedBy=multi-user.target`** — arranca en el boot, sin necesidad de login.

---

## 4. Operación del servicio

Todos los comandos se ejecutan **en la Pi** y requieren `sudo`:

```bash
# Estado y salud
sudo systemctl status mcpato          # ¿activo? PID, uptime, últimas líneas
sudo systemctl is-active mcpato       # active / inactive / failed
sudo systemctl is-enabled mcpato      # enabled = arranca en el boot

# Arrancar / parar / reiniciar
sudo systemctl start mcpato
sudo systemctl stop mcpato
sudo systemctl restart mcpato         # tras cambiar .env o desplegar binario nuevo

# Boot automático
sudo systemctl enable mcpato          # activar arranque en el boot (ya hecho)
sudo systemctl disable mcpato         # desactivarlo

# Logs (van al journal de systemd, no a un archivo)
sudo journalctl -u mcpato -f          # en vivo (pagos de funding, transiciones)
sudo journalctl -u mcpato -n 100      # últimas 100 líneas
sudo journalctl -u mcpato --since "1 hour ago"
```

Tras editar el unit (`/etc/systemd/system/mcpato.service`):

```bash
sudo systemctl daemon-reload
sudo systemctl restart mcpato
```

---

## 5. Dashboard web (carry)

- URL: **http://raspberrypi.local:8090** (o `http://<ip-pi>:8090`)
- Muestra: equity, Δ vs inicial, **funding acumulado**, retorno anualizado
  estimado, pagos cobrados y estado de la posición (ABIERTA / EN CASH).
- Configurado en `.env`:
  - `MCPATO_HTTP_BIND=0.0.0.0` → accesible desde cualquier equipo de la LAN.
    Para restringirlo solo a la Pi, cambiar a `127.0.0.1` y reiniciar.
  - `MCPATO_HTTP_PORT=8090` → **se cambió del 8080 por defecto** porque en esta
    Pi el 8080 ya lo ocupa otro proceso (la red de contenedores Podman rootless,
    `pasta`). Si el 8080 está libre en tu equipo, puede volver a usarse.

Variables propias del carry (en `.env`, con sus defaults): `MCPATO_CARRY_LEVERAGE`
(1.0), `MCPATO_CARRY_POLL_SECS` (300), `MCPATO_CARRY_NEG_WINDOW` (9),
`MCPATO_CARRY_EXIT_THR` (-0.00005), `MCPATO_CARRY_ENTRY_THR` (0.00005). Detalle en
[`docs/CARRY_BOT.md`](./docs/CARRY_BOT.md).

---

## 6. Reconstruir / actualizar el binario

El binario es específico de ARM64; se compila **en la propia Pi** (cross-compilar
desde el Mac es más complejo). Para desplegar cambios de código:

```bash
# 1) En el Mac: sincronizar fuente (excluye target/, data/ y .env)
rsync -az --delete \
  --exclude='/target' --exclude='/data' --exclude='.env' --exclude='.DS_Store' \
  -e ssh ./ <usuario>@raspberrypi.local:/home/<usuario>/mcpato/

# 2) En la Pi: recompilar y reiniciar
ssh <usuario>@raspberrypi.local
cd ~/mcpato
source ~/.cargo/env          # cargo no está en el PATH por defecto en sesiones no-login
cargo build --release        # ~20s incremental (deps cacheadas); ~4 min en limpio
sudo systemctl restart mcpato
```

> **Importante:** el `rsync` **excluye `.env`** para no pisar la config de la Pi
> (puerto, bind, `MCPATO_CARRY_BOT`, token). El `.env` se gestiona a mano en la
> Pi. El primer build completo tardó ~4 min; los incrementales, segundos.

---

## 7. Resiliencia (dos niveles)

1. **Dentro del proceso** (`src/carry.rs`): el funding se lee por **REST con
   reintento** en cada ciclo; un corte de red no tumba el bot (no hay sockets
   persistentes que se queden zombi). Al volver la red, recoge las liquidaciones
   pendientes desde la última cobrada.
2. **A nivel de sistema** (systemd): si el proceso muere o la Pi se reinicia,
   `Restart=always` lo levanta de nuevo y continúa desde el estado de SQLite.

---

## 8. Backup y rollback

El estado vive en `data/mcpato.sqlite`. Para respaldarlo al Mac:

```bash
# Parar primero para no copiar la BD a medio escribir (recomendado)
ssh <usuario>@raspberrypi.local 'sudo systemctl stop mcpato'
scp <usuario>@raspberrypi.local:/home/<usuario>/mcpato/data/mcpato.sqlite ./backup-mcpato-$(date +%F).sqlite
ssh <usuario>@raspberrypi.local 'sudo systemctl start mcpato'
```

**Rollback al organismo evolutivo** (si alguna vez se quisiera): en la Pi, quitar
`MCPATO_CARRY_BOT=true` del `.env`, restaurar el `.bak` sobre `mcpato.sqlite`
(porque el binario nuevo cambió el formato del genoma y no carga el estado viejo
del carry para el organismo), y `sudo systemctl restart mcpato`.

---

## 9. Acceso SSH

Instala la clave pública del Mac en la Pi para entrar **sin contraseña**:

```bash
ssh-copy-id <usuario>@raspberrypi.local   # pide la contraseña una sola vez
ssh <usuario>@raspberrypi.local           # ya entra con clave
```

`sudo` en la Pi sí pide la contraseña del usuario (no se documenta aquí).
