# Team driver strength in Standings

In a team race, the iRating cell can show up to three green upward chevrons or
three red downward chevrons. They compare the active driver with known drivers
of that team. The strongest gets three upward chevrons, the weakest three
downward chevrons, and intermediate ranks receive fewer. A single known driver
has no comparative grade; equal values receive equal grades.

Hover over the cell to see the active driver's rank, the evidence used, and the
known teammates' iRatings and available clean-lap averages. The list identifies
the active driver and how many complete clean stints support each average.

## Discovering the team

The SDK's `DriverInfo.Drivers` contains the currently seated driver for each car
entry. It replaces that row when drivers change; the inspected data does not
include the full inactive team roster. The overlay therefore caches drivers as
they take the seat, using a positive `TeamID` and `UserID`. Solo entries with
`TeamID: 0` cannot be combined into a fictitious team.

The hover says that this is a comparison of drivers seen so far. It cannot
promise to include a teammate who has never appeared in this overlay's session.
The cache is local to this run and session; it is not a downloaded official
roster or a shared crew-history service.

## Changing from iRating to pace

Initially the chevrons rank usable iRatings. Once at least two known teammates
have each completed a qualifying stint, drivers with measured pace are ranked
by clean-lap average. A driver without that pace evidence continues to use the
iRating comparison, with the basis stated in the hover. Ratings and seconds are
never combined into a single numerical ranking.

A qualifying stint needs a known start, continuous driver/scoring coverage and
at least five clean laps. A witnessed stop or a validated active-driver change
can close it. Scoring laps can arrive while the car is `NotInWorld`: local
rendering is not required. A missing sequence of lap results or a receiver gap
prevents the partial stint from being presented as complete.

Pit and caution laps are excluded where observed, with a robust lap-time filter
removing remaining large outliers. A driver's first partial stint after late
attachment does not qualify. On a swap, the outgoing stint closes before the
incoming driver's lap baseline is established; the old result cannot be credited
to the new driver. Same-driver refuelling stops allow subsequent stints to be
measured too.

These are clean session averages, not a measurement of innate driver skill.
Traffic, fuel load and changing track conditions can still affect them.

The public data does not reveal rival fuel loads. A speculative fuel-stop
estimate never becomes a completed driver-stint measurement by itself.
