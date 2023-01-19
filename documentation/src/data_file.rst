Statistics
##########

:Author: Tolan Blundell

Data file
=========

Centinela stores statistics about event counts over time. The file is specified as the second argument
when starting Centinela. The file is in JSON format. It's not pretty-printed so you may want to pipe it through jq to
view it. The data file is written every 30 seconds as it can become quite large. On startup it is read from disk.
