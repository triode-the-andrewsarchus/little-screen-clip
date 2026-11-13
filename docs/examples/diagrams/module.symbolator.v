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
