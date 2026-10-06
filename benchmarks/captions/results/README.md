One file per run of `bench.sh`, named after the commit measured and the
mode. Each line: job, wall time, CPU time, peak resident memory.

`1b388fd-many.txt`: the many-clips mode on a 4-core Xeon at 2.1 GHz with
15 GB, 6 fps, x264 ultrafast. `still-2h` is 2,666 html captions over two
hours at 1920x1080, `still-2m` the first 45 of them. `validate` is
`geneva validate` on the two-hour document. Before that commit every
markup clip's prepared document and picture stayed in memory for the
whole render, about 33 MB a clip at 1080p.
