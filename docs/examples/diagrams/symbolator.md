# Symbolator previews

Local source and rendered views of the same diagrams.

## entity.symbolator.vhdl

```symbolator
library ieee;
use ieee.std_logic_1164.all;

entity sample_counter is
  generic (
    WIDTH : positive := 8
  );
  port (
    --# {{clocks|Clock}}
    clk : in std_logic;
    --# {{control|Control}}
    reset_n : in std_logic;
    enable : in std_logic;
    --# {{data|Data}}
    count : out std_logic_vector(WIDTH-1 downto 0);
    bus_io : inout std_logic_vector(3 downto 0)
  );
end entity sample_counter;
```

## component.symbolator.vhdl

```symbolator
library ieee;
use ieee.std_logic_1164.all;

package preview_components is
  component mixer
    generic (WIDTH : positive := 16);
    port (
      clk : in std_logic;
      left_in, right_in : in std_logic_vector(WIDTH-1 downto 0);
      result : out std_logic_vector(WIDTH downto 0)
    );
  end component mixer;
end package;
```

## module.symbolator.v

```symbolator
module packet_buffer #(parameter WIDTH = 8) (input wire clk,
  //# {{control|Control}}
  input wire reset_n,
  input wire enable,
  //# {{data|Data}}
  input wire [WIDTH-1:0] data_in,
  output reg [WIDTH-1:0] data_out,
  inout wire [3:0] bus_io
);
endmodule
```
