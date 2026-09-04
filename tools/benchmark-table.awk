BEGIN {
    FS = "\t"
}

NR == 1 { next }

{
    key = $1 SUBSEP $2
    count[key]++
    elapsed[key, count[key]] = $4
    rss[key, count[key]] = $5
    if (!(key in max_rss) || $5 > max_rss[key]) max_rss[key] = $5
}

function sorted_value(values, key, samples, rank, copy, i, j, value) {
    for (i = 1; i <= samples; i++) copy[i] = values[key, i]
    for (i = 2; i <= samples; i++) {
        value = copy[i]
        j = i - 1
        while (j >= 1 && copy[j] > value) {
            copy[j + 1] = copy[j]
            j--
        }
        copy[j + 1] = value
    }
    return copy[rank]
}

function median(values, key, samples, middle) {
    middle = int(samples / 2)
    if (samples % 2) return sorted_value(values, key, samples, middle + 1)
    return (sorted_value(values, key, samples, middle) + \
        sorted_value(values, key, samples, middle + 1)) / 2
}

END {
    scenarios[1] = "single"
    scenarios[2] = "tree"
    modes[1] = "gnu-rm"
    modes[2] = "delegate"
    modes[3] = "trash"
    for (s = 1; s <= 2; s++) {
        baseline_key = scenarios[s] SUBSEP "gnu-rm"
        baseline = median(elapsed, baseline_key, count[baseline_key])
        for (m = 1; m <= 3; m++) {
            key = scenarios[s] SUBSEP modes[m]
            median_ns = median(elapsed, key, count[key])
            p05 = int((count[key] * 5 + 99) / 100)
            p95 = int((count[key] * 95 + 99) / 100)
            printf "| %s | %s | %.3f | %.3f-%.3f | %.3f | %.3f | %.2fx |\n", \
                scenarios[s], modes[m], median_ns / 1000000, \
                sorted_value(elapsed, key, count[key], p05) / 1000000, \
                sorted_value(elapsed, key, count[key], p95) / 1000000, \
                median(rss, key, count[key]) / 1024, max_rss[key] / 1024, \
                median_ns / baseline
        }
    }
}
