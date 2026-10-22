# Grafana dashboards

Importable dashboards for the Prometheus metrics that `surtr serve --metrics`
exposes.

| File | Shows |
|---|---|
| `surtr-overview.json` | Health, CPU load and temperature, memory, GPU temperature and power |
| `surtr-ai-workstation.json` | Per GPU: load, VRAM, power against its limit, temperatures, clocks, and what limits the clock (throttle reasons) |

## Use

1. Run Surtr with metrics on each machine:

   ```sh
   surtr serve --metrics 127.0.0.1:9469
   ```

   The address is loopback only. To scrape from another machine, run
   Prometheus (or Grafana Alloy) on the same machine, or reach it through an
   SSH tunnel; `--metrics-remote` exists but the endpoint has no
   authentication.

2. Add a scrape job to Prometheus:

   ```yaml
   scrape_configs:
     - job_name: surtr
       scrape_interval: 10s
       static_configs:
         - targets: ["127.0.0.1:9469"]
   ```

3. In Grafana, **Dashboards → New → Import** and upload a JSON file. Pick
   your Prometheus data source; the dashboards use the scrape's `instance`
   label to tell machines apart.

The port in these examples is arbitrary; Surtr has no default metrics port
yet.

## Metric names

All metrics start with `surtr_` and use base units: `surtr_temperature_celsius`,
`surtr_power_watts`, `surtr_voltage_volts`, `surtr_current_amperes`,
`surtr_clock_hertz`, `surtr_load_ratio` (0 to 1), `surtr_fan_rpm`,
`surtr_fan_ratio`, `surtr_memory_used_bytes`, `surtr_energy_joules`, plus
`surtr_gpu_throttle_active` (0 or 1 per reason), `surtr_health_level`
(0 ok, 1 warning, 2 critical) and `surtr_info`. Labels: `provider`, `device`,
`sensor`, `label`.

A test in `crates/surtr-export` checks that every query in these files uses a
metric Surtr exports.
