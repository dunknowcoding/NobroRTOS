#include <NobroAvrNano.h>

nobro::AvrNanoApp app;
nobro::AvrNanoDeadline deadline;
nobro::AvrNanoGpio led(LED_BUILTIN);
nobro::AvrNanoInterrupt irq(2);
nobro::AvrNanoPwm pwm(3);
nobro::AvrNanoAdc adc(A0);
nobro::AvrNanoI2c i2c;
nobro::AvrNanoSpi spi(10);
nobro::AvrNanoUart uart;

volatile bool exercise = false;

void onEdge() {}

void setup() {
  nobro::TaskId sense = app.sensor("sense", 10);
  nobro::TaskId control = app.control("control", 20);
  nobro::TaskId report = app.service("report", 100);
  nobro::TaskId health = app.service("health", 500);
  app.wire(sense, control).wire(control, report).wire(health, report);
  if (!app.admit() || !exercise) return;

  bool level = false;
  uint16_t sample = 0;
  uint8_t tx[1] = {0};
  uint8_t rx[1] = {0};
  deadline.armAfterUs(1000);
  deadline.due();
  deadline.cancel();
  led.begin(OUTPUT);
  led.write(true);
  led.read(level);
  irq.attach(onEdge, CHANGE);
  irq.detach();
  pwm.begin();
  pwm.setDuty(128);
  adc.begin();
  adc.read(sample);
  i2c.begin();
  i2c.probe(0x68);
  spi.begin();
  spi.transfer(tx, rx, 1);
  uart.begin();
  uart.write(tx, 1);
  nobro::AvrNanoPower::cooperativeIdle();
  if (exercise) nobro::AvrNanoReset::restartApplication();
}

void loop() {}
