#!/bin/bash
grub-install \
	--force-extra-removable \
	--bootloader-id="ubuntu"
update-grub
