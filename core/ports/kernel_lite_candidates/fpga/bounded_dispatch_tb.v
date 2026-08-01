`timescale 1ns/1ps
module bounded_dispatch_tb;
    reg clock = 0;
    reg reset_n = 0;
    reg [3:0] ready = 0;
    wire valid;
    wire [1:0] selected;
    integer seen = 0;

    nobro_bounded_dispatch dut(
        .clock(clock), .reset_n(reset_n), .ready(ready),
        .valid(valid), .selected(selected)
    );
    always #5 clock = ~clock;
    always @(posedge clock) if (valid) seen = seen | (1 << selected);

    initial begin
        #12 reset_n = 1;
        ready = 4'b1111;
        #80;
        if (seen !== 15) $fatal(1, "bounded dispatcher starved a ready task");
        ready = 0;
        #20;
        if (valid !== 0) $fatal(1, "dispatcher asserted valid without work");
        $display("NOBRO-FPGA tasks=4 fairness=1 idle=1 all_pass=1");
        $finish;
    end
endmodule
