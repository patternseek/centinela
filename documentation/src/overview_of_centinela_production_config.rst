Production config
#################

:Author: Tolan Blundell

An overview of the config file structure for Centinela.

global
======

General config for Centinela

global.notifiers_for_files_last_seen
------------------------------------

The notifier ID to which Centinela should send a periodic message summarising which files it is watching and when it
last saw a new line in each one.

global.period_for_files_last_seen
---------------------------------

How often to send the files last seen message, in seconds.

file_sets
=========

Sets of files to monitor. Each key is the ID for a file set.

file_sets.<file set id>.file_globs
----------------------------------

A list of file path globs which used to match files to be included in the current file set.

file_sets.<file set id>.monitor_notifier_sets
---------------------------------------------

A list outlining which monitors (by ID) to enable for this file set, and which notifiers (by ID) to use for
each of those monitors.

For example:

.. code-block:: yaml
    monitor_notifier_sets:
      gnome_events: ~
      service_start:
        - service_changes


This enables two monitors, gnome_events and service_restart. The gnome_events monitor will track events but send no
notifications, whereas the service_start monitor will send events via the service_changes notifier.