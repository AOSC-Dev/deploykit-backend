#!/bin/bash

# Enable error trapping.
set -Eeuo pipefail

# HO-Z6000G does not support booting via ID.
grub-install \
	--force-extra-removable

# Update GRUB.
update-grub
