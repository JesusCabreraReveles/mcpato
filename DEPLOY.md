# Despliegue en Raspberry Pi (entrenamiento continuo)

Documentación del despliegue de **mcPato** en una Raspberry Pi de la red local,
corriendo como servicio `systemd` para entrenarse de forma permanente.

> Los datos personales (usuario, IP, contraseñas) se omiten a propósito.
> Sustituye los placeholders `<usuario>` y `<ip-pi>` por los tuyos.
>
> Última actualización: 2026-06-24

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
| **Binario** | `/home/<usuario>/mcpato/target/release/mcpato` (release ARM64, ~9.7 MB) |
| **Servicio** | `mcpato.service` (systemd, habilitado en arranque) |
| **Dashboard** | http://raspberrypi.local:8090 (accesible desde la LAN) |
| **Instrumento** | BTCUSDT, timeframe 5m, notificaciones Telegram activas |

---

## 2. Estructura en disco y permisos

Todo el proyecto pertenece al usuario de despliegue (`<usuario>`):

```
/home/<usuario>/mcpato/                 drwxr-xr-x
├── src/                                fuente Rust
├── target/release/mcpato               -rwxrwxr-x   (binario compilado)
├── data/                               drwxrwxr-x
│   └── mcpato.sqlite                   -rw-r--r--   (estado + histórico, persistente)
├── .env                                -rw-r--r--   (config + token Telegram — NO en git)
├── Cargo.toml / Cargo.lock
└── ...

/etc/systemd/system/mcpato.service      -rw-r--r--  root:root   (unit del servicio)
```

Notas:
- **`data/mcpato.sqlite`** es el estado vivo (equity, posición, generaciones,
  histórico de velas, señales). Sobrevive a reinicios → el bot continúa donde
  lo dejó. Es lo único que conviene respaldar.
- **`.env`** contiene el token de Telegram; está en `.gitignore` y se copió
  manualmente (no viene del repo). **No se sube a GitHub.**
- El **unit** es propiedad de `root` porque vive en `/etc/systemd/system`.

---

## 3. El servicio systemd

Archivo: `/etc/systemd/system/mcpato.service` (sustituye `<usuario>`):

```ini
[Unit]
Description=mcPato - bot de trading evolutivo (entrenamiento continuo)
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

Claves del diseño:
- **`WorkingDirectory`** — imprescindible: la app lee `.env` y escribe
  `data/mcpato.sqlite` con rutas relativas al directorio de trabajo.
- **`Restart=always` + `RestartSec=5`** — se reinicia solo tras cualquier salida
  (crash, muerte del organismo, fallo persistente). Como recarga el estado de
  SQLite, retoma el entrenamiento.
- **`StartLimitIntervalSec=0`** — desactiva el límite de reintentos de systemd,
  para que `Restart=always` nunca quede bloqueado.
- **`After/Wants network-online.target`** — espera a la red antes de arrancar
  (el backfill inicial necesita la API REST de Binance).
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
sudo journalctl -u mcpato -f          # en vivo (señales, generaciones, warns)
sudo journalctl -u mcpato -n 100      # últimas 100 líneas
sudo journalctl -u mcpato --since "1 hour ago"
```

Tras editar el unit (`/etc/systemd/system/mcpato.service`):

```bash
sudo systemctl daemon-reload
sudo systemctl restart mcpato
```

---

## 5. Dashboard web

- URL: **http://raspberrypi.local:8090** (o `http://<ip-pi>:8090`)
- Configurado en `.env`:
  - `MCPATO_HTTP_BIND=0.0.0.0` → accesible desde cualquier equipo de la LAN.
    Para restringirlo solo a la Pi, cambiar a `127.0.0.1` y reiniciar.
  - `MCPATO_HTTP_PORT=8090` → **se cambió del 8080 por defecto** porque en esta
    Pi el 8080 ya lo ocupa otro proceso (la red de contenedores Podman rootless,
    `pasta`). Si el 8080 está libre en tu equipo, puede volver a usarse.

---

## 6. Reconstruir / actualizar el binario

El binario es específico de ARM64; se compila **en la propia Pi** (cross-compilar
desde el Mac es más complejo). Para desplegar cambios de código:

```bash
# 1) En el Mac: sincronizar fuente (excluye target/, data/ y secretos)
rsync -az --delete \
  --exclude='/target' --exclude='/data' --exclude='.DS_Store' \
  -e ssh ./ <usuario>@raspberrypi.local:/home/<usuario>/mcpato/

# 2) En la Pi: recompilar y reiniciar
ssh <usuario>@raspberrypi.local
cd ~/mcpato
source ~/.cargo/env          # cargo no está en el PATH por defecto en sesiones no-login
cargo build --release        # ~4 min en esta Pi
sudo systemctl restart mcpato
```

> Nota: el `rsync` de arriba **no** sincroniza `.env` si lo excluyes; cópialo a
> mano una sola vez y no lo metas en git. El primer build completo tardó
> **4m 12s**; los incrementales son mucho más rápidos.

---

## 7. Resiliencia (dos niveles)

1. **Dentro del proceso** (`src/ws_binance.rs`): si se cae la red o la conexión
   queda zombi, el stream detecta el silencio (timeout de lectura de 190s),
   reconecta y **rellena por REST las velas perdidas** del hueco. Responde a los
   pings de Binance para mantener viva la conexión. Un mensaje malformado se
   ignora en vez de tumbar el daemon.
2. **A nivel de sistema** (systemd): si el proceso muere o la Pi se reinicia,
   `Restart=always` lo levanta de nuevo y continúa desde el estado de SQLite.

---

## 8. Backup del estado de entrenamiento

El progreso vive en `data/mcpato.sqlite`. Para respaldarlo al Mac:

```bash
# Parar primero para no copiar la BD a medio escribir (recomendado)
ssh <usuario>@raspberrypi.local 'sudo systemctl stop mcpato'
scp <usuario>@raspberrypi.local:/home/<usuario>/mcpato/data/mcpato.sqlite ./backup-mcpato-$(date +%F).sqlite
ssh <usuario>@raspberrypi.local 'sudo systemctl start mcpato'
```

---

## 9. Acceso SSH

Instala la clave pública del Mac en la Pi para entrar **sin contraseña**:

```bash
ssh-copy-id <usuario>@raspberrypi.local   # pide la contraseña una sola vez
ssh <usuario>@raspberrypi.local           # ya entra con clave
```

`sudo` en la Pi sí pide la contraseña del usuario (no se documenta aquí).
