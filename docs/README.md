# Documentación de mcPato

- **[EXPERIMENTOS.md](./EXPERIMENTOS.md)** — Bitácora completa de la búsqueda de
  edge: qué se probó (predicción single-asset, cross-sectional, carry), los
  resultados de cada experimento, las trampas evitadas (sobreajuste, Sharpe ciego
  a colas, cherry-picking) y cómo se llegó a la conclusión de que **el carry de
  BTC es la única estrategia con renta real**. Incluye cómo reproducir cada
  experimento.

- **[../DEPLOY.md](../DEPLOY.md)** — Despliegue del daemon en Raspberry Pi como
  servicio systemd (directorio, permisos, operación, backup).

- **[CARRY_BOT.md](./CARRY_BOT.md)** — Diseño y operación del bot de carry
  (delta-neutral), la conclusión accionable de la bitácora: cómo funciona, sus
  variables de entorno, expectativas honestas y siguientes pasos.
