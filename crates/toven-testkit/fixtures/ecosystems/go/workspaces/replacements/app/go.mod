module example.com/app

go 1.27

require (
	example.com/core v0.0.0
	example.com/external v1.0.0
)

replace example.com/core => ../core

replace example.com/external => example.com/published v1.4.2
