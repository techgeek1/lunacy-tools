# Lunacy Tools
A toolset for working with documents created in [Lunacy](https://icons8.com/lunacy) by Icons8.

More tools will be added over time as the need arises.

Available Tools:
 - A color palette generator similar to [tints.dev](https://tints.dev) for quickly iterating
   on color schemes.

## Color Palette Generator
Colors can be generated via the command line, or a `.json` file describing the colors to generate.
Each color will have 9 tints generated in steps from 100-900. By default the color value
is assumed to be at step 500 but this can be overriden by specifying `"step": <value>` in the 
json file.

Colors are linked by name and if an existing color is found, the color is updated rather
than replaced, allowing for iteration on color palettes without breaking existing pages.

An example color json file is shown below. Color names must be unique, any number of colors
can be added.
```
{
    "dark"      : { "value": "#121212" },
    "light"     : { "value": "#f2f2f2" },
    "pink"      : { "value": "#C92ABB" },
    "purple"    : { "value": "#720DA5" },
    "green"     : { "value": "#99F915" },
    "blue"      : { "value": "#3714AE" }
}
```

Should you wish to automate the process, a command line interface is also provided for
specifying colors. Use `--color=<name>:<value>` to specify colors. Multiple colors can be 
specified with multiple arguments, or a semicolon separated list of color pairs.

For more information on usage, use `--help`.
