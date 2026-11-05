# Publishing `k1roshio/octane-driver-rearm` to Packagist

Packagist requires `composer.json` at the **repository root**. This monorepo keeps the package under `packages/octane-driver/`, so submit a **dedicated** public Git repo whose root is that package (not the whole Rearm monorepo).

## One-time: create the package repo

From the monorepo root (WSL/Linux):

```bash
./scripts/prepare-packagist-repo.sh
cd octane-driver-rearm
git init
git add .
git commit -m "Initial release of k1roshio/octane-driver-rearm"
# Create public GitHub repo k1roshio/octane-driver-rearm, then:
git branch -M main
git remote add origin https://github.com/k1roshio/octane-driver-rearm.git
git push -u origin main
git tag v0.1.0
git push origin v0.1.0
```

`prepare-packagist-repo.sh` syncs `packages/octane-driver` → `octane-driver-rearm/` (gitignored in the monorepo; has its own README for the public repo).

## Submit on Packagist

1. Sign in at [packagist.org](https://packagist.org/) with GitHub.
2. **Submit** → paste the package repo URL (`https://github.com/k1roshio/octane-driver-rearm`).
3. Enable the GitHub Service / Packagist webhook so new tags auto-update.
4. After approval, users install with:

```bash
composer require k1roshio/octane-driver-rearm
```

## What stays in the monorepo

The product repo (`rearm-php` / Rearm) still contains Rust crates, Docker, and a **path** copy of the driver for development. Bump versions in both places when you release (or automate with a subtree split later).

## Checklist before Submit

- [ ] `composer.json` → `"name": "k1roshio/octane-driver-rearm"`, license, autoload, `extra.laravel.providers`
- [ ] Public GitHub/GitLab repo
- [ ] At least one version tag (`v0.1.0`)
- [ ] README documents `octane:start --server=rearm` / `rearm:start`
- [ ] No `vendor/` committed
