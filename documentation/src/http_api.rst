Statistics
##########

:Author: Tolan Blundell

HTTP API
========

Centinela starts an HTTP server listening on port 8694. The port isn't currently configurable.

The following endpoints are available:

GET /fileset
------------

Get a list of the file set IDs being monitored.

GET /fileset/{fileset_id}/monitor
---------------------------------

Get a list of monitor IDs active for {fileset_id}.

GET /fileset/{fileset_id}/monitor/{monitor_id}
----------------------------------------------

Get the monitor data for the monitor {monitor_id} watching the file set {fileset_id}.

GET /dump
---------

Dump the entire in-memory data for Centinela.