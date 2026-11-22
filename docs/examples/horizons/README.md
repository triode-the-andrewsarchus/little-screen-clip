# JPL Horizons trajectories

Open each text file in Little Screen Clip Files to plot its path and scrub through the recorded dates. The samples are heliocentric positions from [NASA/JPL Horizons](https://ssd.jpl.nasa.gov/horizons/).

| File | Coverage | Sampling |
| --- | --- | --- |
| [1P/Halley](1P-Halley.txt) | 1985–2062 return orbit | 10 days |
| [99942 Apophis](99942-Apophis.txt) | 2028–2031, including the 2029 Earth flyby | 2 days |
| [3I/ATLAS](3I-ATLAS.txt) | 2024–2029 interstellar passage | 14 days |
| [4 Vesta](4-Vesta.txt) | 2026–2029 main-belt orbit | 30 days, plain-text vectors |
| [Mars](Mars-ICRF.json) | January 1–3, 2024 TDB | 1 day, API JSON in ICRF |

To export another path, select **Vector Table** in Horizons and set **Coordinate Center** to the Sun body center (`500@10`). Use J2000 ecliptic or ICRF axes, geometric XYZ positions, and AU or kilometer units. Save the full output including its header and `$$SOE` / `$$EOE` markers, or save the API response as JSON.

The viewer interpolates between the supplied positions. Choose a finer sample interval for encounters or other fast changes in direction.

See the [Solar System import guide](../solar-system/README.md) for CCSDS OEM and simulation CSV examples.
